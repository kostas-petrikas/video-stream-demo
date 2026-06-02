use error_stack::{Report, ResultExt};
pub use s2n_quic::Client;
use s2n_quic::{
    Connection,
    client::Connect,
    stream::{BidirectionalStream, ReceiveStream, SendStream},
};
use signal_proto::{MessageStreamWrite, Request};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use webrtc::peer_connection::{RTCIceCandidateInit, RTCSessionDescription};

#[derive(Debug, thiserror::Error)]
#[error("Signaling connection error")]
pub struct Error;

#[derive(Debug)]
pub struct SignalingConnection {
    pub connection: Connection,
    pub stream: BidirectionalStream,
}

pub type SignalingReceiver = ReceiveStream;

#[derive(Clone)]
pub struct SignalingSender(Arc<(Connection, Mutex<SendStream>)>);

impl SignalingSender {
    pub async fn send_offer(
        &self,
        client_id: u32,
        offer: RTCSessionDescription,
    ) -> Result<(), Report<Error>> {
        self.0
            .1
            .lock()
            .await
            .send_message(&Request::WebRTCOffer {
                client_id,
                payload: offer,
            })
            .await
            .change_context(Error)
    }

    pub async fn send_answer(
        &self,
        client_id: u32,
        answer: RTCSessionDescription,
    ) -> Result<(), Report<Error>> {
        self.0
            .1
            .lock()
            .await
            .send_message(&Request::WebRTCAnswer {
                client_id,
                payload: answer,
            })
            .await
            .change_context(Error)
    }

    pub async fn send_candidate(
        &self,
        client_id: u32,
        mut candidate: RTCIceCandidateInit,
    ) -> Result<(), Report<Error>> {
        candidate.url = Some(String::new());

        self.0
            .1
            .lock()
            .await
            .send_message(&Request::ICECandidate {
                client_id,
                candidate,
            })
            .await
            .change_context(Error)
    }
}

impl SignalingConnection {
    pub async fn connect(
        client: &Client,
        addr: SocketAddr,
        server_name: String,
    ) -> Result<Self, Report<Error>> {
        let mut connection = client
            .connect(Connect::new(addr).with_server_name(server_name))
            .await
            .change_context(Error)?;

        connection.keep_alive(true).change_context(Error)?;

        let stream = connection
            .open_bidirectional_stream()
            .await
            .change_context(Error)?;

        Ok(Self { connection, stream })
    }

    pub fn split(self) -> (SignalingSender, SignalingReceiver) {
        let Self { connection, stream } = self;

        let (rx, tx) = stream.split();

        (SignalingSender(Arc::new((connection, Mutex::new(tx)))), rx)
    }
}
