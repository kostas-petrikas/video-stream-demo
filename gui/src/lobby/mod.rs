mod signaling;
use signal_proto::{ClientRef, MessageStreamWrite, Request, Response, into_response_stream};
use std::net::SocketAddr;
use std::sync::Arc;

use iced::{
    Alignment, Element,
    Length::{Fill, FillPortion},
    Task,
    widget::{button, column, container, row, text},
};

use crate::video_call::{self, VideoCall};

#[derive(Clone, Debug)]
pub enum Message {
    Connect {
        addr: SocketAddr,
        name: String,
    },
    Connected {
        connection: Arc<signaling::SignalingConnection>,
    },
    Members(Vec<ClientRef>),
    VideoCall(video_call::Message),
    Error(String),
}

pub enum Lobby {
    Disconnected {
        client: signaling::Client,
    },
    Connecting {
        client: signaling::Client,
        name: String,
    },
    Connected {
        client: signaling::Client,
        name: String,
        sender: signaling::SignalingSender,
        members: Vec<ClientRef>,
        video_call: video_call::VideoCall,
    },
}

impl Lobby {
    pub fn new() -> Self {
        let client = signaling::Client::builder()
            .with_io("0.0.0.0:0")
            .unwrap()
            .with_tls(certs::CERT_PEM)
            .unwrap()
            .start()
            .unwrap();

        Self::Disconnected { client }
    }

    pub fn update(&mut self, msg: Message) -> Task<Message> {
        match self {
            Self::Disconnected { client } => match msg {
                Message::Connect { addr, name } => {
                    let client = client.clone();
                    *self = Self::Connecting {
                        client: client.clone(),
                        name: name.clone(),
                    };

                    return Task::future(async move {
                        match signaling::SignalingConnection::connect(&client, addr, "demo".into())
                            .await
                        {
                            Ok(mut connection) => {
                                if let Err(err) = connection
                                    .stream
                                    .send_message(&Request::SetLabel(name))
                                    .await
                                {
                                    return Message::Error(format!("{err}"));
                                }
                                Message::Connected {
                                    connection: Arc::new(connection),
                                }
                            }
                            Err(err) => Message::Error(format!("{err}")),
                        }
                    });
                }
                _ => {}
            },
            Self::Connecting { client, name } => match msg {
                Message::Connected { connection } => {
                    let connection =
                        Arc::try_unwrap(connection).expect("Should hold exclusive connection");
                    let (sender, receiver) = connection.split();

                    *self = Self::Connected {
                        client: client.clone(),
                        name: name.clone(),
                        sender,
                        members: Vec::new(),
                        video_call: video_call::VideoCall::new(),
                    };

                    // translate signaling responses to events
                    return Task::run(into_response_stream(receiver), |res| match res {
                        Err(err) => Message::Error(format!("{err}")),
                        Ok(response) => match response {
                            Response::ClientList(members) => Message::Members(members),
                            Response::WebRTCOffer { client_id, payload } => {
                                Message::VideoCall(video_call::Message::GotOffer {
                                    client_id,
                                    offer: payload,
                                })
                            }
                            Response::WebRTCAnswer { client_id, payload } => {
                                Message::VideoCall(video_call::Message::GotAnswer {
                                    client_id,
                                    answer: payload,
                                })
                            }
                            Response::ICECandidate {
                                client_id,
                                candidate,
                            } => Message::VideoCall(video_call::Message::GotICECandidate {
                                client_id,
                                candidate,
                            }),
                        },
                    });
                }
                _ => {}
            },
            Self::Connected {
                client: _,
                name: _,
                sender,
                members,
                video_call,
            } => match msg {
                Message::Members(new_members) => {
                    *members = new_members;
                }
                Message::Error(error) => log::error!("{error}"),
                Message::VideoCall(video_call_msg) => match video_call_msg {
                    video_call::Message::SendOffer(offer) => match video_call {
                        VideoCall::Connecting {
                            client_id,
                            ctx: _,
                            connection: _,
                        } => {
                            let sender = sender.clone();
                            let client_id = *client_id;

                            return Task::future(async move {
                                match sender.send_offer(client_id, offer).await {
                                    Ok(_) => Message::VideoCall(video_call::Message::OfferSent),
                                    Err(err) => {
                                        log::error!("{err:?}");
                                        Message::VideoCall(video_call::Message::Disconnected)
                                    }
                                }
                            });
                        }
                        _ => {}
                    },
                    video_call::Message::SendAnswer(answer) => match video_call {
                        VideoCall::Connecting {
                            client_id,
                            ctx: _,
                            connection: _,
                        } => {
                            let sender = sender.clone();
                            let client_id = *client_id;

                            return Task::future(async move {
                                match sender.send_answer(client_id, answer).await {
                                    Ok(_) => Message::VideoCall(video_call::Message::AnswerSent),
                                    Err(err) => {
                                        log::error!("{err:?}");
                                        Message::VideoCall(video_call::Message::Disconnected)
                                    }
                                }
                            });
                        }
                        _ => {}
                    },
                    video_call::Message::SendICECandidate(candidate) => match video_call {
                        VideoCall::Connecting {
                            client_id,
                            ctx: _,
                            connection: _,
                        } => {
                            let sender = sender.clone();
                            let client_id = *client_id;

                            return Task::future(async move {
                                match sender.send_candidate(client_id, candidate).await {
                                    Ok(_) => Message::VideoCall(video_call::Message::AnswerSent),
                                    Err(err) => {
                                        log::error!("{err:?}");
                                        Message::VideoCall(video_call::Message::Disconnected)
                                    }
                                }
                            });
                        }
                        _ => {}
                    },
                    _ => return video_call.update(video_call_msg).map(Message::VideoCall),
                },
                _ => {}
            },
        }
        Task::none()
    }

    pub fn view<'a>(&'a self) -> Element<'a, Message> {
        match self {
            Lobby::Disconnected { client: _ } => text("disconnected").into(),
            Lobby::Connecting { client: _, name: _ } => text("connecting").into(),
            Lobby::Connected {
                client: _,
                name,
                sender: _,
                members,
                video_call,
            } => Self::connected_lobby(name, members, video_call),
        }
    }

    pub fn connected_lobby<'a>(
        name: &'a str,
        members: &'a [ClientRef],
        video_call: &'a video_call::VideoCall,
    ) -> Element<'a, Message> {
        let mut users = column![text(format!("👤 {name}"))].padding(10);
        let active_caller = video_call.active_caller();

        for member in members {
            users = users.push(
                row![
                    text(format!("👤 {}", &member.label)).width(Fill),
                    if let Some(caller) = active_caller
                        && caller == member.id
                    {
                        button("🚫").on_press(Message::VideoCall(video_call::Message::Disconnect))
                    } else {
                        button("🤙")
                            .on_press(Message::VideoCall(video_call::Message::Call(member.id)))
                    }
                ]
                .align_y(Alignment::Center),
            )
        }

        row![
            container(users)
                .style(container::bordered_box)
                .width(FillPortion(1))
                .height(Fill),
            container(video_call.view().map(Message::VideoCall))
                .width(FillPortion(3))
                .height(Fill)
                .align_x(Alignment::Center)
        ]
        .into()
    }
}
