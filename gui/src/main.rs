pub mod lobby;
pub mod signin;
pub mod video_call;

use iced::{Element, Task};

use crate::lobby::Lobby;

pub fn main() {
    env_logger::init();
    iced::application(DemoApp::init, DemoApp::update, DemoApp::view)
        .title("WebRTC demo")
        .theme(iced::Theme::KanagawaDragon)
        .window_size((740, 360))
        .run()
        .unwrap();
}

#[derive(Clone, Debug)]
enum Message {
    Signin(signin::Message),
    Lobby(lobby::Message),
}

enum DemoApp {
    Signin(signin::Signin),
    Lobby(lobby::Lobby),
}

impl DemoApp {
    pub fn init() -> (Self, Task<Message>) {
        let this = Self::Signin(Default::default());

        // no background tasks
        (this, Task::none())
    }

    pub fn view<'a>(&'a self) -> Element<'a, Message> {
        match self {
            Self::Signin(signin) => signin.view().map(Message::Signin),
            Self::Lobby(lobby) => lobby.view().map(Message::Lobby),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        log::debug!("{message:?}");
        match self {
            Self::Signin(signin) => match message {
                Message::Signin(signin_msg) => {
                    if let signin::Message::Signin = &signin_msg {
                        if let Some(ctx) = signin.validate() {
                            *self = Self::Lobby(Lobby::new());
                            return Task::done(Message::Lobby(lobby::Message::Connect {
                                addr: ctx.server,
                                name: ctx.name,
                            }));
                        }
                    }

                    return signin.update(signin_msg).map(Message::Signin);
                }
                _ => {}
            },
            Self::Lobby(lobby) => match message {
                Message::Lobby(lobby_msg) => return lobby.update(lobby_msg).map(Message::Lobby),
                _ => {}
            },
        }

        Task::none()
    }
}
