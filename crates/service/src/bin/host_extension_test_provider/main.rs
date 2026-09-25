//! Test-side native Provider fixture for user-extensible host extensions.
//!
//! This binary is the open-compute-owned reference implementation of the Provider side of the
//! extension session protocol. It mirrors what any operator-built Provider does: read `OCP2`
//! attach requests that carry one passed session file descriptor over the control socket
//! (its standard input), echo each attach nonce in its ACK, and then serve the
//! `HostExtension` Cap'n Proto interface on that session directly to workerd. The fixture
//! implements directory listing (`call`) and file reading (`openStream` plus `read`) over
//! relative paths rooted at its working directory so product Gates can exercise the full
//! `ocd -> workerd -> Provider` call path end to end.
//!
//! The protocol schema in this directory is copied from the workerd fork pin; see its header
//! for the regeneration command. The binary is a `test-support` target: it is never built or
//! shipped as part of the production daemon.

#[rustfmt::skip]
#[allow(
    unused_qualifications,
    missing_debug_implementations,
    missing_docs,
    dead_code,
    unreachable_pub,
    elided_lifetimes_in_paths,
    unused_must_use,
    reason = "committed Cap'n Proto codegen is not maintained by hand"
)]
#[allow(
    clippy::allow_attributes_without_reason,
    clippy::semicolon_if_nothing_returned,
    clippy::uninlined_format_args,
    clippy::doc_markdown,
    clippy::trivially_copy_pass_by_ref,
    clippy::cloned_instead_of_copied,
    clippy::type_complexity,
    clippy::too_many_arguments,
    reason = "committed Cap'n Proto codegen is not maintained by hand"
)]
pub mod host_extension_capnp {
    include!("host_extension_capnp_fixture.rs");
}

use std::cell::RefCell;
use std::ffi::CString;
use std::fs::File;
use std::io::{IoSliceMut, Read};
use std::os::fd::AsFd as _;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::process::ExitCode;
use std::rc::Rc as CapnpRc;

use capnp_rpc::twoparty;
use host_extension_capnp::{host_extension, host_extension_stream};
use rustix::fs::{AtFlags, Mode, OFlags, Stat};
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

const PROVIDER_ATTACH: &[u8; 4] = b"OCP2";
const ATTACH_NONCE_BYTES: usize = 16;
const LIST_METHOD: u32 = 1;
const READ_METHOD: u32 = 2;
const MAX_PATH_BYTES: usize = 4096;
const MAX_FILE_BYTES: i64 = 64 * 1024 * 1024;
const MAX_READ_BYTES: u32 = 64 * 1024;

/// Serves the session-scoped `HostExtension` interface on one attached descriptor.
struct FileProvider;

/// Serves one `HostExtensionStream` over a descriptor opened by `openStream`.
struct FileStream {
    fd: RefCell<Option<File>>,
}

fn failed(message: &'static str) -> capnp::Error {
    capnp::Error::failed(message.to_string())
}

fn is_regular_file(stat: &Stat) -> bool {
    rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::RegularFile
}

/// Opens `path` relative to the provider working directory with the same fail-closed rules as
/// the protocol reference implementation: every component is resolved with an explicit
/// directory walk that never follows symlinks, and `.` or `..` components never resolve.
fn open_relative(payload: &[u8], directory: bool) -> Result<OwnedFd, capnp::Error> {
    if payload.is_empty() || payload.len() > MAX_PATH_BYTES {
        return Err(failed("invalid fixture path"));
    }
    if payload[0] == b'/' || payload.contains(&b'\\') || payload.contains(&0) {
        return Err(failed("invalid fixture path"));
    }
    let components: Vec<&[u8]> = payload.split(|byte| *byte == b'/').collect();
    if components
        .iter()
        .any(|component| component.is_empty() || *component == b"." || *component == b"..")
    {
        return Err(failed("invalid fixture path"));
    }
    let root = rustix::fs::open(
        ".",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY,
        Mode::empty(),
    )
    .map_err(|_| failed("fixture path unavailable"))?;
    let last = components.len() - 1;
    let mut parent = root;
    for (index, component) in components.iter().enumerate() {
        let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        if index != last || directory {
            flags |= OFlags::DIRECTORY;
        }
        let name = CString::new(*component).map_err(|_| failed("invalid fixture path"))?;
        parent = rustix::fs::openat(&parent, &name, flags, Mode::empty())
            .map_err(|_| failed("fixture path unavailable"))?;
    }
    Ok(parent)
}

impl host_extension::Server for FileProvider {
    async fn call(
        self: CapnpRc<Self>,
        params: host_extension::CallParams,
        mut results: host_extension::CallResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        if params.get_method() != LIST_METHOD {
            return Err(failed("unsupported fixture method"));
        }
        let fd = open_relative(params.get_payload()?, true)?;
        let dir =
            rustix::fs::Dir::read_from(&fd).map_err(|_| failed("fixture directory unavailable"))?;
        let mut names: Vec<Vec<u8>> = Vec::new();
        for entry in dir {
            let entry = entry.map_err(|_| failed("fixture directory unavailable"))?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            let stat = rustix::fs::statat(&fd, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|_| failed("fixture directory unavailable"))?;
            if is_regular_file(&stat) {
                names.push(name.to_vec());
            }
        }
        names.sort();
        let mut listing: Vec<u8> = Vec::new();
        for name in &names {
            listing.extend_from_slice(name);
            listing.push(b'\n');
        }
        results.get().set_payload(&listing);
        Ok(())
    }

    async fn open_stream(
        self: CapnpRc<Self>,
        params: host_extension::OpenStreamParams,
        mut results: host_extension::OpenStreamResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        if params.get_method() != READ_METHOD {
            return Err(failed("unsupported fixture method"));
        }
        let fd = open_relative(params.get_payload()?, false)?;
        let stat = rustix::fs::fstat(&fd).map_err(|_| failed("fixture file unavailable"))?;
        if !is_regular_file(&stat) || stat.st_size > MAX_FILE_BYTES {
            return Err(failed("fixture file unavailable"));
        }
        let stream: host_extension_stream::Client = capnp_rpc::new_client(FileStream {
            fd: RefCell::new(Some(File::from(fd))),
        });
        results.get().set_stream(stream);
        Ok(())
    }
}

impl host_extension_stream::Server for FileStream {
    async fn read(
        self: CapnpRc<Self>,
        params: host_extension_stream::ReadParams,
        mut results: host_extension_stream::ReadResults,
    ) -> Result<(), capnp::Error> {
        let max_bytes = params.get()?.get_max_bytes();
        if max_bytes == 0 || max_bytes > MAX_READ_BYTES {
            return Err(failed("invalid fixture read size"));
        }
        let mut fd = self.fd.borrow_mut();
        let Some(file) = fd.as_mut() else {
            return Err(failed("fixture stream canceled"));
        };
        let mut buffer = vec![0u8; max_bytes as usize];
        let amount = file
            .read(&mut buffer)
            .map_err(|_| failed("fixture read failed"))?;
        let mut results = results.get();
        results.set_payload(&buffer[..amount]);
        results.set_eof(amount == 0);
        Ok(())
    }

    async fn cancel(
        self: CapnpRc<Self>,
        _params: host_extension_stream::CancelParams,
        _results: host_extension_stream::CancelResults,
    ) -> Result<(), capnp::Error> {
        self.fd.borrow_mut().take();
        Ok(())
    }
}

/// Serves one Cap'n Proto RPC session on the attached descriptor until workerd disconnects.
async fn run_session(fd: OwnedFd) {
    let stream = StdUnixStream::from(fd);
    if stream.set_nonblocking(true).is_err() {
        return;
    }
    let Ok(stream) = tokio::net::UnixStream::from_std(stream) else {
        return;
    };
    let (reader, writer) = stream.into_split();
    let network = twoparty::VatNetwork::new(
        reader.compat(),
        writer.compat_write(),
        capnp_rpc::rpc_twoparty_capnp::Side::Server,
        capnp::message::ReaderOptions::default(),
    );
    let client: host_extension::Client = capnp_rpc::new_client(FileProvider);
    let rpc = capnp_rpc::RpcSystem::new(Box::new(network), Some(client.client));
    if let Err(error) = rpc.await
        && error.kind != capnp::ErrorKind::Disconnected
    {
        eprintln!("host-extension-test-provider session failed: {error}");
    }
}

/// Writes the whole buffer to `fd`, retrying on interruption and brief unavailability.
fn write_full(fd: BorrowedFd<'_>, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut written = 0;
    while written < bytes.len() {
        match rustix::io::write(fd, &bytes[written..]) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(amount) => written += amount,
            Err(rustix::io::Errno::INTR | rustix::io::Errno::WOULDBLOCK) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn read_full(fd: BorrowedFd<'_>, bytes: &mut [u8]) -> Result<(), std::io::Error> {
    let mut read = 0;
    while read < bytes.len() {
        match rustix::io::read(fd, &mut bytes[read..]) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(amount) => read += amount,
            Err(rustix::io::Errno::INTR | rustix::io::Errno::WOULDBLOCK) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Reads attach requests on the control socket until `ocd` closes it. Each valid request
/// carries exactly one session descriptor; the fixture hands the descriptor to the session
/// driver and echoes the nonce in its ACK, mirroring the reference implementation that
/// enqueues the session before acknowledging. Workerd buffers its RPC frames until the
/// session's Cap'n Proto server starts draining the socket.
fn control_loop(sessions: &SessionSink) -> Result<(), std::io::Error> {
    let stdin = std::io::stdin();
    let stdin_lock = stdin.lock();
    let control = stdin_lock.as_fd();
    loop {
        let mut frame = [0u8; PROVIDER_ATTACH.len() + ATTACH_NONCE_BYTES];
        let mut slices = [IoSliceMut::new(&mut frame)];
        let mut space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let received = loop {
            match recvmsg(control, &mut slices, &mut ancillary, RecvFlags::empty()) {
                Ok(received) => break received,
                Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
        };
        if received.bytes == 0 {
            return Ok(());
        }
        let passed: Vec<OwnedFd> = ancillary
            .drain()
            .flat_map(|message| match message {
                RecvAncillaryMessage::ScmRights(fds) => fds.collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect();
        if passed.len() != 1 || received.bytes > frame.len() {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        read_full(control, &mut frame[received.bytes..])?;
        if frame[..PROVIDER_ATTACH.len()] != *PROVIDER_ATTACH {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid host extension attach",
            ));
        }
        let fd = passed.into_iter().next().expect("exactly one descriptor");
        sessions.send(fd).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "session driver gone")
        })?;
        let mut ack = [0u8; 1 + ATTACH_NONCE_BYTES];
        ack[1..].copy_from_slice(&frame[PROVIDER_ATTACH.len()..]);
        write_full(control, &ack)?;
        // Test-only fault: leave a stale ACK before the next attach on this Provider.
        if std::fs::remove_file(".ocd-test-duplicate-ack-once").is_ok() {
            write_full(control, &ack)?;
        }
    }
}

/// Sends attached session descriptors from the blocking control loop to the local task that
/// drives their Cap'n Proto servers.
type SessionSink = tokio::sync::mpsc::UnboundedSender<OwnedFd>;

fn main() -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::FAILURE;
    };
    let (sessions, mut session_source) = tokio::sync::mpsc::unbounded_channel::<OwnedFd>();
    let local = tokio::task::LocalSet::new();
    let result = local.block_on(&runtime, async {
        let control = tokio::task::spawn_blocking(move || control_loop(&sessions));
        while let Some(fd) = session_source.recv().await {
            tokio::task::spawn_local(run_session(fd));
        }
        control.await
    });
    match result {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(error)) => {
            eprintln!("host-extension-test-provider: {error}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("host-extension-test-provider: control task failed: {error}");
            ExitCode::FAILURE
        }
    }
}
