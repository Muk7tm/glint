// SPDX-License-Identifier: MIT
//! The lab's control connection to the engine, read by a thread of its own as the window's is.
//!
//! The engine broadcasts every change of state to every client, and lets one go that falls a
//! full outbox behind rather than wait for it (§10). A scenario can run for hours between two
//! requests while its focus changes or warnings pile up there, so nothing here waits for a
//! request before reading.

use std::io::{self, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glint_ipc::{Client, Envelope, Event, ReadError, Request, decode, encode, read_line};

use crate::{LabError, engine_err};

type Reply = Result<Envelope<Event>, ReadError>;

pub struct Link {
    writer: UnixStream,
    next_id: u64,
    replies: Receiver<Reply>,
    reader: Option<JoinHandle<()>>,
}

impl Link {
    /// Takes over a connection that has said hello.
    pub fn new(client: Client) -> Result<Self, LabError> {
        let (mut reader, writer, next_id) = client.into_parts();
        // The thread waits as long as it takes; dropping the link ends the wait.
        reader.get_ref().set_read_timeout(None).map_err(engine_err)?;
        let (tx, replies) = mpsc::channel();
        let reader = std::thread::Builder::new()
            .name("lab-link".into())
            .spawn(move || read(&mut reader, &tx))
            .map_err(engine_err)?;
        Ok(Self {
            writer,
            next_id,
            replies,
            reader: Some(reader),
        })
    }

    /// Sends `request` and waits at most `timeout` for its reply.
    pub fn call(&mut self, request: Request, timeout: Duration) -> Result<Event, ReadError> {
        let id = self.next_id;
        self.next_id += 1;
        self.writer
            .write_all(encode(&Envelope::new(Some(id), request)).as_bytes())?;
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .replies
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Ok(reply)) if reply.id == Some(id) => return Ok(reply.body),
                // The late reply to an earlier request that timed out.
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(e),
                Err(RecvTimeoutError::Timeout) => return Err(io::Error::from(io::ErrorKind::TimedOut).into()),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
                }
            }
        }
    }
}

/// Passes replies on and drops broadcasts: the lab asks for the state when it needs it.
fn read(reader: &mut BufReader<UnixStream>, replies: &Sender<Reply>) {
    loop {
        let reply = match read_line(reader) {
            Ok(Some(line)) => decode::<Event>(&line),
            // The engine closed the connection: calls from now on say so.
            Ok(None) => return,
            Err(e) => Err(e),
        };
        let failed = reply.is_err();
        if reply.as_ref().is_ok_and(|r| r.id.is_none()) {
            continue;
        }
        if replies.send(reply).is_err() || failed {
            return;
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(Shutdown::Both);
        if let Some(t) = self.reader.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::BufRead;
    use std::os::unix::net::UnixListener;

    use glint_ipc::ClientKind;

    use super::*;

    /// An engine that answers every request with a pong, in a socket of its own. After the
    /// hello it broadcasts `broadcasts` times, more than any socket buffer holds, and like the
    /// real one it lets the client go when a write can't get through within a second.
    fn fake_engine(name: &str, broadcasts: usize) -> (std::path::PathBuf, JoinHandle<()>) {
        let dir = std::env::temp_dir().join(format!("glint-link-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{name}.sock"));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let engine = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            writer.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
            let mut lines = BufReader::new(stream).lines();
            let answer = |writer: &mut UnixStream, line: String| {
                let request = decode::<Request>(&line).unwrap();
                let reply = Envelope::new(request.id, Event::Pong);
                writer.write_all(encode(&reply).as_bytes()).map(|()| request.body)
            };
            let Ok(_) = answer(&mut writer, lines.next().unwrap().unwrap()) else {
                return;
            };
            let broadcast = encode(&Envelope::new(None, Event::Bye));
            for _ in 0..broadcasts {
                if writer.write_all(broadcast.as_bytes()).is_err() {
                    return;
                }
            }
            for line in lines {
                let Ok(line) = line else { return };
                if !matches!(answer(&mut writer, line), Ok(r) if r != Request::Quit) {
                    return;
                }
            }
        });
        (path, engine)
    }

    fn connect(path: &std::path::Path) -> Client {
        let mut client = Client::connect(path).unwrap();
        client.hello(ClientKind::Lab, Duration::from_secs(5)).unwrap();
        client
    }

    #[test]
    fn broadcasts_are_read_while_no_request_waits() {
        let (path, engine) = fake_engine("read", 100_000);
        let mut link = Link::new(connect(&path)).unwrap();
        // A scenario busy with the screen for a while.
        std::thread::sleep(Duration::from_secs(2));
        assert_eq!(link.call(Request::Ping, Duration::from_secs(20)).unwrap(), Event::Pong);
        assert_eq!(link.call(Request::Quit, Duration::from_secs(5)).unwrap(), Event::Pong);
        engine.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_client_that_reads_only_while_it_waits_is_let_go() {
        // What the lab did before: the engine gave up on it while it was busy.
        let (path, engine) = fake_engine("unread", 100_000);
        let mut client = connect(&path);
        std::thread::sleep(Duration::from_secs(2));
        let e = client
            .call(Request::Ping, Duration::from_secs(20))
            .unwrap_err()
            .to_string();
        assert!(e.contains("end of file") || e.contains("Broken pipe"), "{e}");
        engine.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_closed_connection_ends_every_call() {
        let (path, engine) = fake_engine("closed", 0);
        let mut link = Link::new(connect(&path)).unwrap();
        assert_eq!(link.call(Request::Quit, Duration::from_secs(5)).unwrap(), Event::Pong);
        engine.join().unwrap();
        let e = link.call(Request::Ping, Duration::from_secs(5)).unwrap_err();
        let e = e.to_string();
        assert!(e.contains("end of file") || e.contains("Broken pipe"), "{e}");
        let _ = std::fs::remove_file(&path);
    }
}
