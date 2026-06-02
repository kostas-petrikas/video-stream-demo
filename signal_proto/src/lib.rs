//! Simple signaling protocol, works over postcard encoding on the wire, all payloads prefixed with u16 size
//! decoding first reads u16 size and expects postcard encoded payload of that size as follow up

use async_stream::stream;
use core::future::Future;
use futures_util::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, Stream};
use postcard::{self, from_bytes, to_allocvec};
pub use rtc::peer_connection::{sdp::RTCSessionDescription, transport::RTCIceCandidateInit};

#[derive(Debug, thiserror::Error)]
pub enum MessageStreamError {
    #[error("Message is too big")]
    TooBig,
    #[error("IO error: {0:?}")]
    Io(std::io::Error),
    #[error("Failed to serialize: {0:?}")]
    Serialize(postcard::Error),
    #[error("Failed to deserialize: {0:?}")]
    Deserialize(postcard::Error),
}

pub trait MessageStreamRead {
    fn recv_message<T>(&mut self) -> impl Future<Output = Result<Option<T>, MessageStreamError>>
    where
        T: serde::de::DeserializeOwned + core::fmt::Debug;
}

pub trait MessageStreamWrite {
    fn send_message<T>(
        &mut self,
        message: &T,
    ) -> impl Future<Output = Result<(), MessageStreamError>>
    where
        T: serde::Serialize + core::fmt::Debug + ?Sized;
}

/// Receiver/decoder for messages
impl<S: AsyncRead + Unpin> MessageStreamRead for S {
    async fn recv_message<T>(&mut self) -> Result<Option<T>, MessageStreamError>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut size_buf = [0u8; 2];
        let mut total_read = 0;

        // Manually read the first 2 bytes so we can catch a clean EOF
        while total_read < 2 {
            let n = self
                .read(&mut size_buf[total_read..])
                .await
                .map_err(MessageStreamError::Io)?;

            if n == 0 {
                if total_read == 0 {
                    // Clean shutdown: Stream closed gracefully before a new message started
                    return Ok(None);
                } else {
                    // Corrupted shutdown: Stream closed halfway through writing the u16 size
                    return Err(MessageStreamError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Stream closed mid-size header",
                    )));
                }
            }
            total_read += n;
        }

        // Convert explicitly from Little Endian safely across architectures
        let size = u16::from_le_bytes(size_buf);
        let mut buff = vec![0u8; size as usize];

        self.read_exact(&mut buff)
            .await
            .map_err(MessageStreamError::Io)?;

        let msg = from_bytes(&buff).map_err(MessageStreamError::Deserialize)?;
        Ok(Some(msg))
    }
}

/// Sender/encoder for messages
impl<S: AsyncWrite + Unpin> MessageStreamWrite for S {
    async fn send_message<T>(&mut self, message: &T) -> Result<(), MessageStreamError>
    where
        T: serde::Serialize + core::fmt::Debug + ?Sized,
    {
        let serialized = to_allocvec(message).map_err(MessageStreamError::Serialize)?;

        if serialized.len() > u16::MAX as _ {
            return Err(MessageStreamError::TooBig);
        }

        let size = serialized.len() as u16;

        // Write using explicit Little Endian bytes
        self.write_all(&size.to_le_bytes())
            .await
            .map_err(MessageStreamError::Io)?;

        self.write_all(&serialized)
            .await
            .map_err(MessageStreamError::Io)?;

        // Ensure s2n-quic flushes the framing down to the network layer
        self.flush().await.map_err(MessageStreamError::Io)?;

        Ok(())
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ClientRef {
    pub label: String,
    pub id: u32,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub enum Response {
    ClientList(Vec<ClientRef>),
    WebRTCOffer {
        client_id: u32,
        payload: RTCSessionDescription,
    },
    WebRTCAnswer {
        client_id: u32,
        payload: RTCSessionDescription,
    },
    ICECandidate {
        client_id: u32,
        candidate: RTCIceCandidateInit,
    },
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub enum Request {
    SetLabel(String),
    WebRTCOffer {
        client_id: u32,
        payload: RTCSessionDescription,
    },
    WebRTCAnswer {
        client_id: u32,
        payload: RTCSessionDescription,
    },
    ICECandidate {
        client_id: u32,
        candidate: RTCIceCandidateInit,
    },
}

pub fn into_response_stream<S>(
    mut source: S,
) -> impl Stream<Item = Result<Response, MessageStreamError>>
where
    S: MessageStreamRead,
{
    stream! {
        loop {
            match source.recv_message().await {
                Ok(Some(msg)) => yield Ok(msg),
                Ok(None) => break, // Break loop gracefully when peer shuts down stream
                Err(e) => {
                    yield Err(e);
                    break;
                }
            }
        }
    }
}
