//! Byte streams for the IPC: Unix domain socket or Windows named pipe.

use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}

pub type Conn = Box<dyn Duplex>;

#[cfg(unix)]
mod imp {
    use super::*;
    use crate::paths;
    use tokio::net::{UnixListener, UnixStream};

    pub struct Listener(UnixListener);

    pub async fn connect() -> io::Result<Conn> {
        Ok(Box::new(UnixStream::connect(paths::socket_path()).await?))
    }

    pub fn bind() -> io::Result<Listener> {
        paths::create_runtime_dir()?;
        let path = paths::socket_path();
        // A socket left by a crashed instance (the caller checked nothing answers).
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Listener(listener))
    }

    impl Listener {
        pub async fn accept(&mut self) -> io::Result<Conn> {
            let (stream, _) = self.0.accept().await?;
            Ok(Box::new(stream))
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::paths;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

    /// ERROR_PIPE_BUSY: every instance is taken; another is created shortly.
    const ERROR_PIPE_BUSY: i32 = 231;

    pub struct Listener {
        next: NamedPipeServer,
    }

    pub async fn connect() -> io::Result<Conn> {
        let name = paths::pipe_name();
        for _ in 0..50 {
            match ClientOptions::new().open(&name) {
                Ok(client) => return Ok(Box::new(client)),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "raudo pipe stays busy"))
    }

    pub fn bind() -> io::Result<Listener> {
        // first_pipe_instance: fails if another process already owns this name.
        let next = ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(paths::pipe_name())?;
        Ok(Listener { next })
    }

    impl Listener {
        pub async fn accept(&mut self) -> io::Result<Conn> {
            self.next.connect().await?;
            let fresh = ServerOptions::new().reject_remote_clients(true).create(paths::pipe_name())?;
            let connected = std::mem::replace(&mut self.next, fresh);
            Ok(Box::new(connected))
        }
    }
}

pub use imp::{bind, connect, Listener};
