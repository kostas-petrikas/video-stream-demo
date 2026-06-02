//! Signaling service to be hosted on stable IP

use async_broadcast::{Receiver, Sender, broadcast};
use error_stack::{Report, ResultExt};
use hashbrown::HashMap;
use s2n_quic::{
    Connection, Server,
    stream::{BidirectionalStream, ReceiveStream, SendStream},
};
use signal_proto::{
    ClientRef, MessageStreamRead, MessageStreamWrite, RTCIceCandidateInit, RTCSessionDescription,
    Request, Response,
};
use std::sync::{Arc, Mutex};
use tokio::select;

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("Signaling server error")]
pub struct SignalingError;

#[derive(Debug, Clone, Default)]
pub struct Clients {
    counter: u32,
    clients: HashMap<u32, Option<String>>,
}

impl Clients {
    pub fn new_client(&mut self) -> u32 {
        self.counter += 1;
        self.clients.insert(self.counter, None);

        self.counter
    }

    pub fn set_label(&mut self, id: u32, label: String) {
        self.clients.insert(id, Some(label));
    }

    pub fn drop_client(&mut self, id: u32) {
        self.clients.remove(&id);
    }

    pub fn list_other_clients(&self, id: u32) -> Vec<signal_proto::ClientRef> {
        self.clients
            .iter()
            .filter_map(|(cid, label)| {
                if cid == &id {
                    return None;
                }

                let Some(label) = label else { return None };

                Some(ClientRef {
                    id: *cid,
                    label: label.clone(),
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub enum Update {
    ClientsChanged,
    Offer {
        from: u32,
        to: u32,
        payload: RTCSessionDescription,
    },
    Answer {
        from: u32,
        to: u32,
        payload: RTCSessionDescription,
    },
    ICECandidate {
        from: u32,
        to: u32,
        candidate: RTCIceCandidateInit,
    },
}

struct SignalingServer {
    server: Server,
    clients: Arc<Mutex<Clients>>,
    update_tx: Sender<Update>,
    update_rx: Receiver<Update>,
}

impl SignalingServer {
    const UPDATE_CAP: usize = 100;

    pub fn new(addr: &str) -> Result<Self, Report<SignalingError>> {
        let server = Server::builder()
            .with_io(addr)
            .change_context(SignalingError)?
            .with_tls((certs::CERT_PEM, certs::KEY_PEM))
            .change_context(SignalingError)?
            .start()
            .change_context(SignalingError)?;
        let clients: Arc<Mutex<Clients>> = Default::default();
        let (mut update_tx, update_rx) = broadcast(Self::UPDATE_CAP);
        update_tx.set_overflow(true);

        Ok(Self {
            server,
            clients,
            update_rx,
            update_tx,
        })
    }

    pub async fn listen(mut self) -> Result<(), Report<SignalingError>> {
        while let Some(connection) = self.server.accept().await {
            let clients = self.clients.clone();
            let update_tx = self.update_tx.clone();
            let update_rx = self.update_rx.new_receiver();

            tokio::spawn(Self::new_connection(
                connection, clients, update_tx, update_rx,
            ));
        }

        Ok(())
    }

    async fn new_connection(
        mut connection: Connection,
        clients: Arc<Mutex<Clients>>,
        update_tx: Sender<Update>,
        update_rx: Receiver<Update>,
    ) -> Result<(), Report<SignalingError>> {
        let stream = connection
            .accept_bidirectional_stream()
            .await
            .change_context(SignalingError)?;

        let stream = match stream {
            Some(stream) => stream,
            _ => return Ok(()),
        };

        let client_id = clients.lock().unwrap().new_client();

        if let Err(err) = ClientHandler::new(
            client_id,
            clients.clone(),
            stream,
            update_tx.clone(),
            update_rx,
        )
        .handle()
        .await
        {
            println!("Client error: {err:?}");
        }

        clients.lock().unwrap().drop_client(client_id);
        update_tx
            .broadcast(Update::ClientsChanged)
            .await
            .change_context(SignalingError)?;
        Ok(())
    }
}

pub struct ClientHandler {
    client_id: u32,
    clients: Arc<Mutex<Clients>>,
    client_tx: SendStream,
    client_rx: Option<ReceiveStream>,
    update_tx: Sender<Update>,
    update_rx: Receiver<Update>,
}

impl ClientHandler {
    pub fn new(
        client_id: u32,
        clients: Arc<Mutex<Clients>>,
        stream: BidirectionalStream,
        update_tx: Sender<Update>,
        update_rx: Receiver<Update>,
    ) -> Self {
        let (client_rx, client_tx) = stream.split();

        Self {
            client_id,
            clients,
            client_tx,
            client_rx: Some(client_rx),
            update_tx,
            update_rx,
        }
    }

    pub async fn handle(&mut self) -> Result<(), Report<SignalingError>> {
        let mut stream = match self.client_rx.take() {
            Some(stream) => stream,
            None => return Ok(()),
        };
        let (tx, mut request_rx) = tokio::sync::mpsc::channel::<Request>(16);

        tokio::spawn(async move {
            loop {
                match stream.recv_message::<Request>().await {
                    Ok(Some(request)) => {
                        if tx.send(request).await.is_err() {
                            break; // Main loop dropped the receiver, shut down safely
                        }
                    }
                    Ok(None) => break, // Clean EOF, stream finished
                    Err(e) => {
                        println!("ERR: {e:?}");
                        break;
                    }
                }
            }
        });

        loop {
            select! {
                request = request_rx.recv() => {
                    match request {
                        Some(request) => self.handle_request(request).await?,
                        None => return Ok(()) // Channel closed means reader task ended (EOF/Error)
                    }
                },
                update = self.update_rx.recv() => {
                    self.handle_update(
                        update.change_context(SignalingError)?
                    ).await?;
                }
            }
        }
    }

    async fn handle_request(&mut self, request: Request) -> Result<(), Report<SignalingError>> {
        match request {
            Request::SetLabel(label) => {
                self.clients
                    .lock()
                    .unwrap()
                    .set_label(self.client_id, label);
                self.update_tx
                    .broadcast(Update::ClientsChanged)
                    .await
                    .change_context(SignalingError)?;
            }
            Request::WebRTCOffer { client_id, payload } => {
                self.update_tx
                    .broadcast(Update::Offer {
                        from: self.client_id,
                        to: client_id,
                        payload,
                    })
                    .await
                    .change_context(SignalingError)?;
            }
            Request::WebRTCAnswer { client_id, payload } => {
                self.update_tx
                    .broadcast(Update::Answer {
                        from: self.client_id,
                        to: client_id,
                        payload,
                    })
                    .await
                    .change_context(SignalingError)?;
            }
            Request::ICECandidate {
                client_id,
                candidate,
            } => {
                self.update_tx
                    .broadcast(Update::ICECandidate {
                        from: self.client_id,
                        to: client_id,
                        candidate,
                    })
                    .await
                    .change_context(SignalingError)?;
            }
        }
        Ok(())
    }

    async fn handle_update(&mut self, update: Update) -> Result<(), Report<SignalingError>> {
        match update {
            Update::ClientsChanged => {
                let clients = self
                    .clients
                    .lock()
                    .unwrap()
                    .list_other_clients(self.client_id);

                self.client_tx
                    .send_message(&Response::ClientList(clients))
                    .await
                    .change_context(SignalingError)?;
            }
            Update::Offer { from, to, payload } => {
                if to != self.client_id {
                    return Ok(());
                }

                self.client_tx
                    .send_message(&Response::WebRTCOffer {
                        client_id: from,
                        payload,
                    })
                    .await
                    .change_context(SignalingError)?;
            }
            Update::Answer { from, to, payload } => {
                if to != self.client_id {
                    return Ok(());
                }

                self.client_tx
                    .send_message(&Response::WebRTCAnswer {
                        client_id: from,
                        payload,
                    })
                    .await
                    .change_context(SignalingError)?;
            }
            Update::ICECandidate {
                from,
                to,
                candidate,
            } => {
                if to != self.client_id {
                    return Ok(());
                }

                self.client_tx
                    .send_message(&Response::ICECandidate {
                        client_id: from,
                        candidate,
                    })
                    .await
                    .change_context(SignalingError)?;
            }
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let server = SignalingServer::new("0.0.0.0:8000").unwrap();
    println!("listening");
    server.listen().await.unwrap();
}
