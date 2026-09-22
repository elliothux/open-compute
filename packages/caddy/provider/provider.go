// Package provider connects Caddy's DNS-01 solver to the ocd challenge authority.
package provider

import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"errors"
	"io"
	"net"
	"path/filepath"
	"strings"
	"time"

	"github.com/caddyserver/caddy/v2"
	"github.com/caddyserver/caddy/v2/caddyconfig/caddyfile"
	"github.com/libdns/libdns"
)

const maxMessage = 1024

var errUnavailable = errors.New("open-compute challenge provider unavailable")

// Provider is the platform-scoped DNS-01 provider.
type Provider struct {
	Socket string `json:"socket"`
}

func init() { caddy.RegisterModule(Provider{}) }

// CaddyModule registers the provider with Caddy.
func (Provider) CaddyModule() caddy.ModuleInfo {
	return caddy.ModuleInfo{
		ID:  "dns.providers.opencompute",
		New: func() caddy.Module { return new(Provider) },
	}
}

// Provision validates the private provider socket path.
func (p *Provider) Provision(caddy.Context) error {
	if !filepath.IsAbs(p.Socket) || strings.ContainsRune(p.Socket, '\x00') {
		return errors.New("open-compute challenge socket path is invalid")
	}
	return nil
}

// UnmarshalCaddyfile accepts `opencompute <absolute-socket-path>`.
func (p *Provider) UnmarshalCaddyfile(d *caddyfile.Dispenser) error {
	d.Next()
	args := d.RemainingArgs()
	if len(args) != 1 {
		return d.ArgErr()
	}
	p.Socket = args[0]
	if d.NextBlock(0) {
		return d.Err("opencompute does not accept a block")
	}
	return nil
}

type request struct {
	Action string `json:"action"`
	Zone   string `json:"zone,omitempty"`
	Value  string `json:"value,omitempty"`
	ID     string `json:"id,omitempty"`
}

type response struct {
	ID      string `json:"id"`
	Deleted bool   `json:"deleted"`
	Error   string `json:"error"`
}

func (p *Provider) exchange(ctx context.Context, input request) (response, error) {
	var result response
	body, err := json.Marshal(input)
	if err != nil || len(body) == 0 || len(body) > maxMessage {
		return result, errUnavailable
	}
	var dialer net.Dialer
	conn, err := dialer.DialContext(ctx, "unix", p.Socket)
	if err != nil {
		return result, errUnavailable
	}
	defer conn.Close()
	deadline := time.Now().Add(5 * time.Second)
	if earlier, ok := ctx.Deadline(); ok && earlier.Before(deadline) {
		deadline = earlier
	}
	if conn.SetDeadline(deadline) != nil {
		return result, errUnavailable
	}
	stopCancel := context.AfterFunc(ctx, func() { _ = conn.SetDeadline(time.Now()) })
	defer stopCancel()
	prefix := []byte{byte(len(body) >> 8), byte(len(body))}
	if _, err := io.Copy(conn, bytes.NewReader(prefix)); err != nil {
		return result, errUnavailable
	}
	if _, err := io.Copy(conn, bytes.NewReader(body)); err != nil {
		return result, errUnavailable
	}
	var length uint16
	if err := binary.Read(conn, binary.BigEndian, &length); err != nil || length == 0 || length > maxMessage {
		return result, errUnavailable
	}
	encoded := make([]byte, length)
	if _, err := io.ReadFull(conn, encoded); err != nil || json.Unmarshal(encoded, &result) != nil || result.Error != "" {
		return response{}, errUnavailable
	}
	return result, nil
}

// AppendRecords publishes only TXT values accepted by ocd's fixed zone allowlist.
func (p *Provider) AppendRecords(ctx context.Context, zone string, records []libdns.Record) ([]libdns.Record, error) {
	prepared := make([]libdns.TXT, 0, len(records))
	for _, record := range records {
		txt, ok := asTXT(record)
		if !ok || txt.Text == "" || len(txt.Text) > 255 || txt.TTL < 0 || txt.TTL > 300*time.Second {
			return nil, errUnavailable
		}
		prepared = append(prepared, txt)
	}
	added := make([]libdns.Record, 0, len(records))
	ids := make([]string, 0, len(prepared))
	for _, txt := range prepared {
		name := strings.ToLower(strings.TrimSuffix(libdns.AbsoluteName(txt.Name, zone), "."))
		result, err := p.exchange(ctx, request{Action: "append", Zone: name, Value: txt.Text})
		if err != nil || result.ID == "" {
			cleanupCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			_, _ = p.exchange(cleanupCtx, request{Action: "delete_exact", Zone: name, Value: txt.Text})
			for _, id := range ids {
				_, _ = p.exchange(cleanupCtx, request{Action: "delete", ID: id})
			}
			cancel()
			return nil, errUnavailable
		}
		txt.TTL = 60 * time.Second
		txt.ProviderData = result.ID
		ids = append(ids, result.ID)
		added = append(added, txt)
	}
	return added, nil
}

// DeleteRecords removes the opaque records returned by AppendRecords.
func (p *Provider) DeleteRecords(ctx context.Context, zone string, records []libdns.Record) ([]libdns.Record, error) {
	deleted := make([]libdns.Record, 0, len(records))
	for _, record := range records {
		txt, ok := asTXT(record)
		if !ok {
			return deleted, errUnavailable
		}
		input := request{Action: "delete_exact", Zone: strings.ToLower(strings.TrimSuffix(libdns.AbsoluteName(txt.Name, zone), ".")), Value: txt.Text}
		if id, ok := txt.ProviderData.(string); ok && id != "" {
			input = request{Action: "delete", ID: id}
		}
		result, err := p.exchange(ctx, input)
		if err != nil {
			return deleted, err
		}
		if result.Deleted {
			deleted = append(deleted, txt)
		}
	}
	return deleted, nil
}

func asTXT(record libdns.Record) (libdns.TXT, bool) {
	if record == nil {
		return libdns.TXT{}, false
	}
	if txt, ok := record.(libdns.TXT); ok {
		return txt, true
	}
	parsed, err := record.RR().Parse()
	if err != nil {
		return libdns.TXT{}, false
	}
	txt, ok := parsed.(libdns.TXT)
	return txt, ok
}

var (
	_ caddy.Provisioner     = (*Provider)(nil)
	_ caddyfile.Unmarshaler = (*Provider)(nil)
	_ libdns.RecordAppender = (*Provider)(nil)
	_ libdns.RecordDeleter  = (*Provider)(nil)
)
