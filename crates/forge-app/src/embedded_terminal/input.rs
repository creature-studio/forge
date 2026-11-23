//! One ordered writer per PTY keeps slow child input off the UI thread.
use std::{
    io::{self, Write},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{sync_channel, SyncSender},
        Arc,
    },
};

const MAX_PENDING_BYTES: usize = 8 * 1024 * 1024;

pub(super) struct InputWriter {
    sender: SyncSender<Vec<u8>>,
    pending: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
    #[cfg(test)]
    capture: Option<std::sync::Mutex<Box<dyn Write + Send>>>,
}

impl InputWriter {
    pub(super) fn new(mut writer: Box<dyn Write + Send>) -> io::Result<Self> {
        let (sender, receiver) = sync_channel::<Vec<u8>>(256);
        let pending = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let worker_pending = pending.clone();
        let worker_closed = closed.clone();
        std::thread::Builder::new()
            .name("forge-pty-write".into())
            .spawn(move || {
                for bytes in receiver {
                    if worker_closed.load(Ordering::Acquire) {
                        break;
                    }
                    let result = writer.write_all(&bytes).and_then(|()| writer.flush());
                    worker_pending.fetch_sub(bytes.len(), Ordering::AcqRel);
                    if result.is_err() {
                        break;
                    }
                }
                worker_closed.store(true, Ordering::Release);
            })?;
        Ok(Self {
            sender,
            pending,
            closed,
            #[cfg(test)]
            capture: None,
        })
    }

    pub(super) fn send(&self, bytes: &[u8]) -> io::Result<()> {
        #[cfg(test)]
        if let Some(capture) = &self.capture {
            return capture.lock().unwrap().write_all(bytes);
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal input closed",
            ));
        }
        // Reserve capacity atomically without relying on the newer try_update API;
        // Forge still supports Rust 1.89, and the writer may drain concurrently.
        let mut pending = self.pending.load(Ordering::Acquire);
        loop {
            let next = pending
                .checked_add(bytes.len())
                .filter(|&size| size <= MAX_PENDING_BYTES)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::WouldBlock, "terminal input queue full")
                })?;
            match self.pending.compare_exchange_weak(
                pending,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => pending = actual,
            }
        }
        if let Err(error) = self.sender.try_send(bytes.to_vec()) {
            self.pending.fetch_sub(bytes.len(), Ordering::AcqRel);
            return Err(match error {
                std::sync::mpsc::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "terminal input queue full")
                }
                std::sync::mpsc::TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "terminal input closed")
                }
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn capture(writer: Box<dyn Write + Send>) -> Self {
        let mut input = Self::new(Box::new(io::sink())).unwrap();
        input.capture = Some(std::sync::Mutex::new(writer));
        input
    }
}

impl Drop for InputWriter {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;

    struct PausedWriter {
        entered: mpsc::Sender<()>,
        resume: mpsc::Receiver<()>,
        output: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for PausedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.entered.send(()).unwrap();
            self.resume.recv().unwrap();
            self.output.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn blocked_child_keeps_input_nonblocking_bounded_and_ordered() {
        let (entered, waiting) = mpsc::channel();
        let (resume, paused) = mpsc::channel();
        let output = Arc::new(Mutex::new(Vec::new()));
        let input = InputWriter::new(Box::new(PausedWriter {
            entered,
            resume: paused,
            output: output.clone(),
        }))
        .unwrap();
        input.send(b"\x1b[200~paste\x1b[201~").unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        input.send(b"key").unwrap();
        assert_eq!(
            input.send(&vec![0; MAX_PENDING_BYTES]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        for _ in 0..255 {
            input.send(b"x").unwrap();
        }
        assert_eq!(
            input.send(b"overflow").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(input.pending.load(Ordering::Acquire), 275);
        resume.send(()).unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(&*output.lock().unwrap(), b"\x1b[200~paste\x1b[201~");
        resume.send(()).unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(&*output.lock().unwrap(), b"\x1b[200~paste\x1b[201~key");
        // Let the in-flight write finish after cancellation. Remaining requests
        // must be discarded without another write to the child.
        drop(input);
        resume.send(()).unwrap();
    }
}
