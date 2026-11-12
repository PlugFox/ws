//! The connection type every endpoint works with: a boxed (plain or TLS)
//! stream that first replays bytes read past the HTTP head, feeds what the
//! client sends into an optional [`ConnLog`], and keeps a handle on the raw
//! TCP socket so endpoints can reset it.

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use bytes::{Buf, Bytes};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::log::{ConnLog, TcpEnd};

/// Any byte stream the server can talk over.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// One accepted client connection, positioned just after its HTTP head.
pub struct Conn {
    io: Box<dyn Io>,
    /// Bytes already read past the head; returned before reading `io`.
    pre: Bytes,
    log: Option<Arc<ConnLog>>,
    /// Duplicate of the TCP socket, used to set `SO_LINGER 0`.
    socket: Option<socket2::Socket>,
    pub peer: Option<SocketAddr>,
    /// True on the TLS ports.
    pub tls: bool,
}

impl Conn {
    pub fn new(
        io: Box<dyn Io>,
        socket: Option<socket2::Socket>,
        peer: Option<SocketAddr>,
        tls: bool,
    ) -> Self {
        Conn {
            io,
            pre: Bytes::new(),
            log: None,
            socket,
            peer,
            tls,
        }
    }

    /// Mutable access to the underlying stream (for reading the head).
    pub fn inner(&mut self) -> &mut Box<dyn Io> {
        &mut self.io
    }

    /// Queues bytes that were read past the head.
    pub fn set_prefix(&mut self, pre: Bytes) {
        self.pre = pre;
    }

    /// Starts logging everything read from now on (including the prefix).
    pub fn set_log(&mut self, log: Arc<ConnLog>) {
        self.log = Some(log);
    }

    /// Closes with `SO_LINGER 0` so the peer sees a TCP RST.
    pub fn reset(self) {
        if let Some(s) = &self.socket {
            let _ = s.set_linger(Some(Duration::ZERO));
        }
        drop(self);
    }

    /// True when `id=` logging is on for this connection.
    pub fn is_logged(&self) -> bool {
        self.log.is_some()
    }

    fn note_read(&self, data: &[u8]) {
        if let Some(log) = &self.log {
            log.feed(data);
        }
    }

    fn note_end(&self, end: TcpEnd) {
        if let Some(log) = &self.log {
            log.tcp_end(end);
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        if let Some(log) = &self.log {
            log.finish();
        }
    }
}

impl AsyncRead for Conn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if !this.pre.is_empty() {
            let n = this.pre.len().min(buf.remaining());
            buf.put_slice(&this.pre[..n]);
            this.note_read(&this.pre[..n]);
            this.pre.advance(n);
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        let wanted = buf.remaining() > 0;
        match Pin::new(&mut this.io).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let new = &buf.filled()[before..];
                if new.is_empty() && wanted {
                    this.note_end(TcpEnd::Fin);
                } else {
                    this.note_read(new);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => {
                this.note_end(match e.kind() {
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted => {
                        TcpEnd::Rst
                    }
                    _ => TcpEnd::Fin,
                });
                Poll::Ready(Err(e))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
