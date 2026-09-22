package provider

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"io"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/libdns/libdns"
)

func TestAppendAndDeleteTXT(t *testing.T) {
	dir, err := os.MkdirTemp("", "oc-caddy-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	socket := filepath.Join(dir, "provider.sock")
	listener, err := net.Listen("unix", socket)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = listener.Close() })
	requests := make(chan request, 4)
	go func() {
		deletedOnce := false
		for i := 0; i < 4; i++ {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			var length uint16
			if binary.Read(conn, binary.BigEndian, &length) != nil || length == 0 || length > maxMessage {
				_ = conn.Close()
				return
			}
			body := make([]byte, length)
			if _, err := io.ReadFull(conn, body); err != nil {
				_ = conn.Close()
				return
			}
			var input request
			if json.Unmarshal(body, &input) != nil {
				_ = conn.Close()
				return
			}
			requests <- input
			removed := input.Action == "delete_exact" || input.Action == "delete" && !deletedOnce
			if input.Action == "delete" {
				deletedOnce = true
			}
			encoded, _ := json.Marshal(response{ID: "challenge-1", Deleted: removed})
			_ = binary.Write(conn, binary.BigEndian, uint16(len(encoded)))
			_, _ = conn.Write(encoded)
			_ = conn.Close()
		}
	}()
	provider := Provider{Socket: socket}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	added, err := provider.AppendRecords(ctx, "example.com.", []libdns.Record{
		libdns.TXT{Name: "_acme-challenge", Text: "token-1"},
	})
	if err != nil || len(added) != 1 {
		t.Fatalf("append: records=%v error=%v", added, err)
	}
	var appended request
	select {
	case appended = <-requests:
	case <-ctx.Done():
		t.Fatal("append request did not reach the provider socket")
	}
	if appended.Action != "append" || appended.Zone != "_acme-challenge.example.com" || appended.Value != "token-1" {
		t.Fatalf("unexpected append request: %+v", appended)
	}
	deleted, err := provider.DeleteRecords(ctx, "example.com.", added)
	if err != nil || len(deleted) != 1 {
		t.Fatalf("delete: records=%v error=%v", deleted, err)
	}
	var removed request
	select {
	case removed = <-requests:
	case <-ctx.Done():
		t.Fatal("delete request did not reach the provider socket")
	}
	if removed.Action != "delete" || removed.ID != "challenge-1" {
		t.Fatalf("unexpected delete request: %+v", removed)
	}
	missing, err := provider.DeleteRecords(ctx, "example.com.", added)
	if err != nil || len(missing) != 0 {
		t.Fatalf("repeated delete must report no removed records: records=%v error=%v", missing, err)
	}
	<-requests
	_, err = provider.DeleteRecords(ctx, "example.com.", []libdns.Record{
		libdns.TXT{Name: "_acme-challenge", Text: "token-1"},
	})
	if err != nil {
		t.Fatalf("delete without opaque ID: %v", err)
	}
	var exact request
	select {
	case exact = <-requests:
	case <-ctx.Done():
		t.Fatal("exact delete request did not reach the provider socket")
	}
	if exact.Action != "delete_exact" || exact.Zone != "_acme-challenge.example.com" || exact.Value != "token-1" {
		t.Fatalf("unexpected exact delete request: %+v", exact)
	}
}

func TestAppendRejectsInvalidTXTBeforeConnecting(t *testing.T) {
	provider := Provider{Socket: filepath.Join(t.TempDir(), "missing.sock")}
	_, err := provider.AppendRecords(context.Background(), "example.com.", []libdns.Record{
		libdns.TXT{Name: "_acme-challenge", Text: ""},
	})
	if err == nil {
		t.Fatal("empty TXT token was accepted")
	}
}

func TestAppendCleansUpAfterContextCancellation(t *testing.T) {
	socket := filepath.Join(t.TempDir(), "provider.sock")
	listener, err := net.Listen("unix", socket)
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	requests := make(chan request, 4)
	go func() {
		for i := 0; i < 4; i++ {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			var length uint16
			if binary.Read(conn, binary.BigEndian, &length) != nil {
				_ = conn.Close()
				return
			}
			body := make([]byte, length)
			if _, err := io.ReadFull(conn, body); err != nil {
				_ = conn.Close()
				return
			}
			var input request
			if json.Unmarshal(body, &input) != nil {
				_ = conn.Close()
				return
			}
			requests <- input
			if i == 1 {
				cancel()
				_ = conn.Close() // The TXT was accepted, but its response was lost.
				continue
			}
			encoded, _ := json.Marshal(response{ID: "first-id", Deleted: true})
			_ = binary.Write(conn, binary.BigEndian, uint16(len(encoded)))
			_, _ = conn.Write(encoded)
			_ = conn.Close()
		}
	}()
	_, err = (&Provider{Socket: socket}).AppendRecords(ctx, "example.com.", []libdns.Record{
		libdns.TXT{Name: "_acme-challenge", Text: "first-token"},
		libdns.TXT{Name: "_acme-challenge", Text: "second-token"},
	})
	if err == nil {
		t.Fatal("cancelled append succeeded")
	}
	for _, action := range []string{"append", "append", "delete_exact", "delete"} {
		select {
		case input := <-requests:
			if input.Action != action ||
				action == "delete_exact" && (input.Zone != "_acme-challenge.example.com" || input.Value != "second-token") ||
				action == "delete" && input.ID != "first-id" {
				t.Fatalf("unexpected cleanup request: %+v", input)
			}
		case <-time.After(2 * time.Second):
			t.Fatal("cancelled append did not clean up both TXT records")
		}
	}
}

func TestExchangeStopsOnContextCancellation(t *testing.T) {
	socket := filepath.Join(t.TempDir(), "provider.sock")
	listener, err := net.Listen("unix", socket)
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	received := make(chan struct{})
	go func() {
		conn, err := listener.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		var length uint16
		if binary.Read(conn, binary.BigEndian, &length) != nil {
			return
		}
		if _, err := io.CopyN(io.Discard, conn, int64(length)); err != nil {
			return
		}
		close(received)
		_, _ = io.Copy(io.Discard, conn)
	}()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	result := make(chan error, 1)
	go func() {
		_, err := (&Provider{Socket: socket}).exchange(ctx, request{Action: "delete", ID: "record"})
		result <- err
	}()
	select {
	case <-received:
	case <-time.After(2 * time.Second):
		t.Fatal("provider request did not reach the socket")
	}
	cancel()
	select {
	case err := <-result:
		if err == nil {
			t.Fatal("cancelled exchange succeeded")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("cancelled exchange did not stop promptly")
	}
}
