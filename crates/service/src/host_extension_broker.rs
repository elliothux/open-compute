//! Private generation broker for local native-extension session sockets.

use crate::local_extensions::LocalExtensionRegistry;
use crate::service_invocations::ServiceInvocationRegistry;
use open_compute_core::{ErrorCode, PlatformError, Redactor};
use open_compute_runtime::{
    HostExtensionBrokerRegistry, PersistentHostProcess, PersistentHostProcessSpec,
};
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
use std::collections::HashMap;
use std::io::IoSlice;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt as _, Interest};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, watch};

const REQUEST_MAGIC: &[u8; 4] = b"OCH1";
const PROVIDER_ATTACH: &[u8; 4] = b"OCP1";
const MAX_SESSION_IDENTITY: usize = 128;
const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);
const PROVIDER_STOP_GRACE: Duration = Duration::from_secs(2);
const PROVIDER_KILL_AFTER: Duration = Duration::from_secs(2);
const MAX_PROVIDER_FAILURES: u32 = 6;

struct Provider {
    process: PersistentHostProcess,
    control: UnixStream,
}

struct ProviderSlot {
    live: Option<Provider>,
    failures: u32,
    retry_at: Instant,
}

impl Default for ProviderSlot {
    fn default() -> Self {
        Self {
            live: None,
            failures: 0,
            retry_at: Instant::now(),
        }
    }
}

/// Owns one provider process per configured extension and brokers direct data sockets.
pub(crate) struct HostExtensionBroker {
    sockets: HostExtensionBrokerRegistry,
    extensions: Arc<LocalExtensionRegistry>,
    invocations: Arc<ServiceInvocationRegistry>,
    work_dirs: HashMap<String, PathBuf>,
    providers: Mutex<HashMap<String, ProviderSlot>>,
    redactor: Redactor,
}

impl std::fmt::Debug for HostExtensionBroker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostExtensionBroker")
            .field("extensions", &self.work_dirs.len())
            .finish_non_exhaustive()
    }
}

impl HostExtensionBroker {
    pub(crate) fn new(
        sockets: HostExtensionBrokerRegistry,
        extensions: Arc<LocalExtensionRegistry>,
        invocations: Arc<ServiceInvocationRegistry>,
        storage: &open_compute_storage::PlatformStorage,
        redactor: Redactor,
    ) -> Result<Self, PlatformError> {
        for (name, path) in storage.data_dir().existing_extension_provider_dirs()? {
            if !extensions.contains(&name) {
                PersistentHostProcess::recover_recorded_orphan(&path.join("provider.lease"))?;
            }
        }
        let work_dirs = extensions
            .names()
            .map(|name| {
                let path = storage.data_dir().prepare_extension_provider_dir(name)?;
                let extension = extensions.get(name).ok_or_else(unavailable)?;
                PersistentHostProcess::recover_orphan(
                    &path.join("provider.lease"),
                    &extension.executable_sha256,
                )?;
                Ok::<_, PlatformError>((name.to_owned(), path))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            sockets,
            extensions,
            invocations,
            work_dirs,
            providers: Mutex::new(HashMap::new()),
            redactor,
        })
    }

    pub(crate) fn socket_registry(&self) -> HostExtensionBrokerRegistry {
        self.sockets.clone()
    }

    pub(crate) async fn run(
        &self,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), PlatformError> {
        let result = loop {
            let socket = tokio::select! {
                () = wait_shutdown(&mut shutdown) => break Ok(()),
                socket = self.sockets.take() => socket.1,
            };
            if socket.set_nonblocking(true).is_err() {
                break Err(unavailable());
            }
            let Ok(socket) = UnixStream::from_std(socket) else {
                break Err(unavailable());
            };
            let served = tokio::select! {
                () = wait_shutdown(&mut shutdown) => break Ok(()),
                result = self.serve_generation(socket) => result,
            };
            if let Err(error) = served {
                break Err(error);
            }
        };
        self.shutdown_providers().await;
        result
    }

    async fn serve_generation(&self, mut socket: UnixStream) -> Result<(), PlatformError> {
        loop {
            let identity = match read_request(&mut socket).await {
                Ok(Some(identity)) => identity,
                Ok(None) => return Ok(()),
                Err(_) => return Err(unavailable()),
            };
            let Some(name) = self.invocations.extension_for_session(&identity) else {
                if send_status(&socket, 1).await.is_err() {
                    return Ok(());
                }
                continue;
            };
            match self.open_session(&identity, &name).await {
                Ok(fd) => {
                    if send_fd(&socket, &fd, &[0]).await.is_err() {
                        return Ok(());
                    }
                }
                Err(_) => {
                    if send_status(&socket, 2).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn open_session(&self, identity: &str, name: &str) -> Result<OwnedFd, PlatformError> {
        // ponytail: one global attach lock; split per provider if extension startup throughput
        // becomes material for the single-machine deployment profile.
        let mut providers = self.providers.lock().await;
        let slot = providers.entry(name.to_owned()).or_default();
        if slot
            .live
            .as_ref()
            .is_some_and(|provider| !provider.process.is_running())
        {
            if let Some(provider) = slot.live.take() {
                provider
                    .process
                    .shutdown(PROVIDER_STOP_GRACE, PROVIDER_KILL_AFTER)
                    .await;
            }
            record_failure(slot);
        }
        if slot.live.is_none() {
            if slot.failures >= MAX_PROVIDER_FAILURES || Instant::now() < slot.retry_at {
                return Err(unavailable());
            }
            match self.start_provider(name) {
                Ok(provider) => slot.live = Some(provider),
                Err(error) => {
                    record_failure(slot);
                    return Err(error);
                }
            }
        }
        let (provider_end, workerd_end) = StdUnixStream::pair().map_err(|_| unavailable())?;
        let provider_fd: OwnedFd = provider_end.into();
        let workerd_fd: OwnedFd = workerd_end.into();
        let provider = slot.live.as_mut().ok_or_else(unavailable)?;
        let attached = send_fd(&provider.control, &provider_fd, PROVIDER_ATTACH).await;
        let mut ack = [1u8];
        let acknowledged = attached.is_ok()
            && tokio::time::timeout(ATTACH_TIMEOUT, provider.control.read_exact(&mut ack))
                .await
                .is_ok_and(|result| result.is_ok())
            && ack[0] == 0;
        if !acknowledged
            || self.invocations.extension_for_session(identity).as_deref() != Some(name)
        {
            if let Some(provider) = slot.live.take() {
                provider
                    .process
                    .shutdown(PROVIDER_STOP_GRACE, PROVIDER_KILL_AFTER)
                    .await;
            }
            record_failure(slot);
            return Err(unavailable());
        }
        slot.failures = 0;
        slot.retry_at = Instant::now();
        Ok(workerd_fd)
    }

    fn start_provider(&self, name: &str) -> Result<Provider, PlatformError> {
        let extension = self.extensions.get(name).ok_or_else(unavailable)?;
        let working_directory = self.work_dirs.get(name).ok_or_else(unavailable)?.clone();
        let (parent, child) = StdUnixStream::pair().map_err(|_| unavailable())?;
        parent.set_nonblocking(true).map_err(|_| unavailable())?;
        let control = UnixStream::from_std(parent).map_err(|_| unavailable())?;
        let process = PersistentHostProcess::spawn(
            &extension.executable,
            PersistentHostProcessSpec {
                args: Vec::new(),
                environment: Vec::new(),
                lease_path: working_directory.join("provider.lease"),
                working_directory,
                control_fd: Some(child.into()),
                binary_sha256: extension.executable_sha256.clone(),
                redactor: self.redactor.clone(),
            },
        )?;
        Ok(Provider { process, control })
    }

    async fn shutdown_providers(&self) {
        let providers = std::mem::take(&mut *self.providers.lock().await);
        futures::future::join_all(providers.into_values().filter_map(|slot| {
            slot.live.map(|provider| {
                provider
                    .process
                    .shutdown(PROVIDER_STOP_GRACE, PROVIDER_KILL_AFTER)
            })
        }))
        .await;
    }
}

fn record_failure(slot: &mut ProviderSlot) {
    slot.failures = slot.failures.saturating_add(1).min(MAX_PROVIDER_FAILURES);
    let delay_ms = 100u64.saturating_mul(1u64 << slot.failures).min(5_000);
    slot.retry_at = Instant::now() + Duration::from_millis(delay_ms);
}

async fn read_request(socket: &mut UnixStream) -> std::io::Result<Option<String>> {
    let mut header = [0u8; 6];
    match socket.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    if &header[..4] != REQUEST_MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid host-extension broker request",
        ));
    }
    let length = usize::from(u16::from_be_bytes([header[4], header[5]]));
    if length == 0 || length > MAX_SESSION_IDENTITY {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid host-extension session identity",
        ));
    }
    let mut identity = vec![0; length];
    socket.read_exact(&mut identity).await?;
    String::from_utf8(identity)
        .map(Some)
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))
}

async fn send_status(socket: &UnixStream, status: u8) -> std::io::Result<()> {
    loop {
        socket.writable().await?;
        match socket.try_write(&[status]) {
            Ok(1) => return Ok(()),
            Ok(_) => return Err(std::io::ErrorKind::WriteZero.into()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
}

async fn send_fd(socket: &UnixStream, fd: &OwnedFd, bytes: &[u8]) -> std::io::Result<()> {
    loop {
        socket.writable().await?;
        let sent = socket.try_io(Interest::WRITABLE, || {
            let fds = [fd.as_fd()];
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
            let mut ancillary = SendAncillaryBuffer::new(&mut space);
            if !ancillary.push(SendAncillaryMessage::ScmRights(&fds)) {
                return Err(std::io::ErrorKind::InvalidInput.into());
            }
            sendmsg(
                socket,
                &[IoSlice::new(bytes)],
                &mut ancillary,
                SendFlags::empty(),
            )
            .map_err(std::io::Error::from)
        });
        match sent {
            Ok(sent) if sent == bytes.len() => return Ok(()),
            Ok(_) => return Err(std::io::ErrorKind::WriteZero.into()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
}

async fn wait_shutdown(shutdown: &mut watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    let _ = shutdown.changed().await;
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ServiceUnavailable,
        "local extension provider is unavailable",
    )
}

/// Real local-extension runtime wiring for integration tests.
#[cfg(feature = "test-support")]
#[derive(Debug)]
pub struct LocalExtensionRuntimeForTest {
    broker: Arc<HostExtensionBroker>,
    invocations: Arc<ServiceInvocationRegistry>,
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), PlatformError>>,
}

#[cfg(feature = "test-support")]
impl LocalExtensionRuntimeForTest {
    /// Load configured extensions and start the production broker path.
    pub fn start(
        configs: &std::collections::BTreeMap<String, open_compute_core::LocalExtensionConfig>,
        storage: &Arc<open_compute_storage::PlatformStorage>,
        pins: open_compute_workers::VersionPins,
    ) -> Result<Self, PlatformError> {
        let extensions = Arc::new(LocalExtensionRegistry::load(configs)?);
        let invocations = Arc::new(
            ServiceInvocationRegistry::new(storage.clone(), pins)
                .with_local_extensions(extensions.clone()),
        );
        let broker = Arc::new(HostExtensionBroker::new(
            HostExtensionBrokerRegistry::new(),
            extensions,
            invocations.clone(),
            storage,
            Redactor::new(),
        )?);
        let (shutdown, receiver) = watch::channel(false);
        let task = tokio::spawn({
            let broker = broker.clone();
            async move { broker.run(receiver).await }
        });
        Ok(Self {
            broker,
            invocations,
            shutdown,
            task,
        })
    }

    /// Invocation authority shared with the binding backend.
    #[must_use]
    pub fn invocations(&self) -> Arc<ServiceInvocationRegistry> {
        self.invocations.clone()
    }

    /// Generation socket registry passed to the real runtime supervisor.
    #[must_use]
    pub fn socket_registry(&self) -> HostExtensionBrokerRegistry {
        self.broker.socket_registry()
    }

    /// Stop the broker and every supervised Provider.
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        self.task.await.unwrap().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_compute_core::clock::SystemClock;
    use open_compute_core::config::DataConfig;
    use open_compute_workers::VersionPins;
    use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
    use std::io::{IoSliceMut, Read as _, Write as _};
    use tokio::io::AsyncWriteExt as _;

    fn empty_broker() -> (tempfile::TempDir, HostExtensionBroker) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("data");
        let storage = Arc::new(
            open_compute_storage::PlatformStorage::bootstrap(
                &DataConfig {
                    path: root.clone(),
                    master_key_file: root.join("keys/master.key"),
                    master_key_env: None,
                    sqlite_busy_timeout_ms: 5_000,
                    free_space_soft_bytes: 1,
                    free_space_hard_bytes: 1,
                },
                &SystemClock,
            )
            .unwrap(),
        );
        let extensions = Arc::new(LocalExtensionRegistry::empty());
        let invocations = Arc::new(ServiceInvocationRegistry::new(
            storage.clone(),
            VersionPins::new(),
        ));
        let broker = HostExtensionBroker::new(
            HostExtensionBrokerRegistry::new(),
            extensions,
            invocations,
            &storage,
            Redactor::new(),
        )
        .unwrap();
        (temporary, broker)
    }

    #[tokio::test]
    async fn broker_frame_and_single_fd_handoff_are_exact() {
        let (request_client, request_server) = StdUnixStream::pair().unwrap();
        request_server.set_nonblocking(true).unwrap();
        let mut request_server = UnixStream::from_std(request_server).unwrap();
        let identity = b"session-identity";
        let mut frame = Vec::from(REQUEST_MAGIC);
        frame.extend_from_slice(&(identity.len() as u16).to_be_bytes());
        frame.extend_from_slice(identity);
        (&request_client).write_all(&frame).unwrap();
        assert_eq!(
            read_request(&mut request_server).await.unwrap().as_deref(),
            Some("session-identity")
        );

        let (sender, receiver) = StdUnixStream::pair().unwrap();
        sender.set_nonblocking(true).unwrap();
        let sender = UnixStream::from_std(sender).unwrap();
        let (passed, mut peer) = StdUnixStream::pair().unwrap();
        send_fd(&sender, &passed.into(), &[0]).await.unwrap();

        let mut byte = [0u8; 1];
        let mut slices = [IoSliceMut::new(&mut byte)];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let received = recvmsg(&receiver, &mut slices, &mut ancillary, RecvFlags::empty()).unwrap();
        assert_eq!(received.bytes, 1);
        let fd = ancillary
            .drain()
            .find_map(|message| match message {
                RecvAncillaryMessage::ScmRights(mut fds) => fds.next(),
                _ => None,
            })
            .unwrap();
        let mut received_socket = StdUnixStream::from(fd);
        received_socket.write_all(b"ok").unwrap();
        let mut payload = [0u8; 2];
        peer.read_exact(&mut payload).unwrap();
        assert_eq!(&payload, b"ok");

        let mut slot = ProviderSlot::default();
        for _ in 0..10 {
            record_failure(&mut slot);
        }
        assert_eq!(slot.failures, MAX_PROVIDER_FAILURES);
    }

    #[tokio::test]
    async fn generation_rejects_unknown_sessions_and_shutdown_is_bounded() {
        let (_temporary, broker) = empty_broker();
        let _registry = broker.socket_registry();
        assert!(format!("{broker:?}").contains("extensions: 0"));
        let (client, server) = StdUnixStream::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        server.set_nonblocking(true).unwrap();
        let mut client = UnixStream::from_std(client).unwrap();
        let server = UnixStream::from_std(server).unwrap();
        let identity = b"unknown-session";
        let mut frame = Vec::from(REQUEST_MAGIC);
        frame.extend_from_slice(&(identity.len() as u16).to_be_bytes());
        frame.extend_from_slice(identity);
        let client_task = async move {
            client.write_all(&frame).await.unwrap();
            let mut status = [0u8];
            client.read_exact(&mut status).await.unwrap();
            assert_eq!(status, [1]);
            client.shutdown().await.unwrap();
        };
        let (served, ()) = tokio::join!(broker.serve_generation(server), client_task);
        served.unwrap();

        let (_shutdown, receiver) = watch::channel(true);
        broker.run(receiver).await.unwrap();
    }
}
