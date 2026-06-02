pub mod wrtc;

use iced::{
    Alignment, Element, Length, Task,
    widget::{container, stack, text},
};
use iced_wgpu_video::{VideoFrame, VideoWidget};
use webrtc::peer_connection::{RTCIceCandidateInit, RTCSessionDescription};

#[derive(Clone)]
pub struct NewVideoFrame(pub VideoFrame);

impl core::fmt::Debug for NewVideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewVideoFrame").finish()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    GotICECandidate {
        client_id: u32,
        candidate: RTCIceCandidateInit,
    },
    SendICECandidate(RTCIceCandidateInit),
    NewICECandidate,
    SendOffer(RTCSessionDescription),
    OfferSent,
    SendAnswer(RTCSessionDescription),
    AnswerSent,
    AnswerReceived,
    GotOffer {
        client_id: u32,
        offer: RTCSessionDescription,
    },
    GotAnswer {
        client_id: u32,
        answer: RTCSessionDescription,
    },
    NewOutConnection {
        client_id: u32,
        connection: wrtc::StatefulPeerConnection,
        offer: RTCSessionDescription,
    },
    NewInConnection {
        client_id: u32,
        connection: wrtc::StatefulPeerConnection,
        answer: RTCSessionDescription,
    },
    NewVideoFrame(NewVideoFrame),
    NewVideoTrack,
    VideoBandwidthStats(f64),
    Call(u32),
    Connected,
    Disconnected,
    Disconnect,
}

pub enum VideoCall {
    Disconnected(wrtc::WebRTCContext),
    Connected {
        client_id: u32,
        ctx: wrtc::WebRTCContext,
        connection: wrtc::StatefulPeerConnection,
        video_frame: Option<VideoFrame>,
        bandwidth: f64,
    },
    Connecting {
        client_id: u32,
        ctx: wrtc::WebRTCContext,
        connection: wrtc::StatefulPeerConnection,
    },
}

impl VideoCall {
    pub fn new() -> Self {
        Self::Disconnected(wrtc::WebRTCContext::new().unwrap())
    }

    pub fn active_caller(&self) -> Option<u32> {
        match self {
            Self::Connected {
                client_id,
                ctx: _,
                connection: _,
                video_frame: _,
                bandwidth: _,
            } => Some(*client_id),
            Self::Connecting {
                client_id,
                ctx: _,
                connection: _,
            } => Some(*client_id),
            _ => None,
        }
    }

    fn center<'a>(element: Element<'a, Message>) -> Element<'a, Message> {
        container(
            container(element)
                .width(Length::Shrink)
                .height(Length::Shrink),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
    }

    pub fn view<'a>(&'a self) -> Element<'a, Message> {
        match self {
            Self::Disconnected(_) => {
                Self::center(text("No active call").style(text::secondary).into())
            }
            Self::Connecting {
                client_id: _,
                ctx: _,
                connection: _s,
            } => Self::center(text("Connecting...").into()),
            Self::Connected {
                client_id: _,
                ctx: _,
                connection: _s,
                video_frame,
                bandwidth,
            } => stack!(
                VideoWidget::new(iced::Length::Fill, iced::Length::Fill, video_frame),
                text(format!("{:.2}KB/s", bandwidth / 1000.0)).size(20)
            )
            .into(),
        }
    }

    pub fn update(&mut self, msg: Message) -> Task<Message> {
        match self {
            Self::Connected {
                client_id: _,
                ctx,
                connection,
                video_frame,
                bandwidth,
            } => match msg {
                Message::Disconnect => {
                    let connection = connection.clone();
                    return Task::future(async move {
                        connection.close().await;
                        Message::Disconnected
                    });
                }
                Message::Disconnected => {
                    *self = VideoCall::Disconnected(ctx.clone());
                }
                Message::NewVideoFrame(new_vf) => *video_frame = Some(new_vf.0),
                Message::VideoBandwidthStats(stats) => {
                    *bandwidth = stats;
                }
                _ => {}
            },
            Self::Disconnected(ctx) => match msg {
                Message::Call(client_id) => {
                    let ctx = ctx.clone();
                    return Task::future(async move {
                        match wrtc::StatefulPeerConnection::initiate(&ctx).await {
                            Ok((connection, offer)) => {
                                return Message::NewOutConnection {
                                    client_id,
                                    connection,
                                    offer,
                                };
                            }
                            Err(err) => {
                                log::error!("{err:?}");
                                return Message::Disconnected;
                            }
                        }
                    });
                }
                Message::NewOutConnection {
                    client_id,
                    connection,
                    offer,
                } => {
                    *self = Self::Connecting {
                        client_id,
                        ctx: ctx.clone(),
                        connection,
                    };

                    return Task::done(Message::SendOffer(offer));
                }
                Message::NewInConnection {
                    client_id,
                    connection,
                    answer,
                } => {
                    *self = Self::Connecting {
                        client_id,
                        ctx: ctx.clone(),
                        connection,
                    };

                    return Task::done(Message::SendAnswer(answer));
                }
                Message::GotOffer { client_id, offer } => {
                    let ctx = ctx.clone();
                    return Task::future(async move {
                        if let Ok((connection, answer)) =
                            wrtc::StatefulPeerConnection::follow(&ctx, offer).await
                        {
                            return Message::NewInConnection {
                                client_id,
                                connection,
                                answer,
                            };
                        }

                        Message::Disconnected
                    });
                }
                _ => {}
            },
            Self::Connecting {
                client_id,
                ctx,
                connection,
            } => match msg {
                Message::Disconnect => {
                    let connection = connection.clone();
                    return Task::future(async move {
                        connection.close().await;
                        Message::Disconnected
                    });
                }
                Message::Disconnected => {
                    *self = VideoCall::Disconnected(ctx.clone());
                }
                Message::Connected => match self {
                    VideoCall::Connecting {
                        client_id,
                        ctx,
                        connection,
                    } => {
                        let track = connection.video_track();
                        let conn = connection.clone();

                        *self = VideoCall::Connected {
                            client_id: *client_id,
                            ctx: ctx.clone(),
                            connection: connection.clone(),
                            video_frame: None,
                            bandwidth: 0.0,
                        };

                        match conn {
                            wrtc::StatefulPeerConnection::Initiator(peer) => {
                                return Task::stream(wrtc::PeerConnectionStream::new(
                                    peer.0.handler.clone(),
                                    track,
                                ))
                                .map(|e| match e {
                                    wrtc::PeerConnectionEvent::Disconnected => {
                                        Message::Disconnected
                                    }
                                    wrtc::PeerConnectionEvent::NewVideoFrame(frame) => {
                                        Message::NewVideoFrame(NewVideoFrame(frame))
                                    }
                                    wrtc::PeerConnectionEvent::BandwidthStats(stats) => {
                                        Message::VideoBandwidthStats(stats)
                                    }
                                });
                            }
                            wrtc::StatefulPeerConnection::Follower(peer) => {
                                return Task::stream(wrtc::PeerConnectionStream::new(
                                    peer.0.handler.clone(),
                                    track,
                                ))
                                .map(|e| match e {
                                    wrtc::PeerConnectionEvent::Disconnected => {
                                        Message::Disconnected
                                    }
                                    wrtc::PeerConnectionEvent::NewVideoFrame(frame) => {
                                        Message::NewVideoFrame(NewVideoFrame(frame))
                                    }
                                    wrtc::PeerConnectionEvent::BandwidthStats(stats) => {
                                        Message::VideoBandwidthStats(stats)
                                    }
                                });
                            }
                        }
                    }
                    _ => {}
                },
                Message::GotAnswer {
                    client_id: remote_client_id,
                    answer,
                } => {
                    if *client_id != remote_client_id {
                        return Task::none();
                    }

                    if let wrtc::StatefulPeerConnection::Initiator(conn) = connection {
                        let conn = conn.clone();

                        return Task::future(async move {
                            match conn.handle_answer(answer).await {
                                Ok(_) => Message::AnswerReceived,
                                Err(err) => {
                                    log::error!("{err:?}");

                                    Message::Disconnected
                                }
                            }
                        });
                    }
                }
                Message::GotICECandidate {
                    client_id: remote_client_id,
                    candidate,
                } => {
                    if *client_id != remote_client_id {
                        return Task::none();
                    }

                    let connection = connection.clone();
                    return Task::future(
                        async move { connection.add_ice_candidate(candidate).await },
                    )
                    .map(|_| Message::NewICECandidate);
                }
                Message::AnswerReceived => {
                    return Task::stream(connection.trickle_connection());
                }
                Message::AnswerSent => {
                    return Task::stream(connection.trickle_connection());
                }
                _ => {}
            },
        }

        Task::none()
    }
}
