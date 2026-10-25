//! Owned loopback HTTP fixture. Never used by production clients or trust policy.
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(crate) const BOUND: Duration = Duration::from_secs(4);

pub(crate) struct Server {
    address: SocketAddr,
    release: mpsc::Sender<()>,
    request: mpsc::Receiver<String>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub(crate) fn start(
        reply: &'static [u8],
        stall: bool,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (release, released) = mpsc::channel();
        let (sent, request) = mpsc::channel();
        let thread = thread::spawn(move || {
            let accept_deadline = Instant::now() + BOUND;
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if released.try_recv().is_ok() || Instant::now() >= accept_deadline {
                            return;
                        }
                        thread::park_timeout(Duration::from_millis(5));
                    }
                    Err(error) => panic!("Owned HTTP fixture accept failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(BOUND)).unwrap();
            stream.set_write_timeout(Some(BOUND)).unwrap();
            let mut bytes = Vec::new();
            let mut byte = [0];
            let deadline = Instant::now() + BOUND;
            while bytes.len() < 8192 && !bytes.ends_with(b"\r\n\r\n") {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return;
                }
                stream.set_read_timeout(Some(remaining)).unwrap();
                match stream.read(&mut byte) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => bytes.push(byte[0]),
                }
            }
            let _ = sent.send(String::from_utf8(bytes).unwrap());
            if let Some(cancel) = cancel {
                cancel.store(true, Ordering::Release);
            }
            // A release event, not a sleep: Drop releases this wait on assertion failure too.
            let _ = stream.write_all(reply);
            if stall {
                let _ = released.recv_timeout(BOUND);
            }
        });
        Self {
            address,
            release,
            request,
            thread: Some(thread),
        }
    }

    pub(crate) fn url(&self) -> String {
        format!("http://{}/immutable", self.address)
    }

    pub(crate) fn request(&self) -> String {
        self.request.recv_timeout(BOUND).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.release.send(());
        if let Some(thread) = self.thread.take() {
            assert!(thread.join().is_ok(), "Owned HTTP fixture thread failed");
        }
    }
}

pub(crate) fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_millis(300))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}
