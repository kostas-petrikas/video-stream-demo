use iced::{
    Alignment, Element,
    Length::{self, FillPortion},
    Task,
    widget::{button, column, container, row, text, text_input},
};
use std::net::SocketAddr;

pub struct ValidatedSignin {
    pub server: SocketAddr,
    pub name: String,
}

#[derive(Clone, Debug)]
pub enum Message {
    ServerAddr(String),
    Name(String),
    Error(String),
    Signin,
}

pub struct Signin {
    server_addr: String,
    name: String,
    error: Option<String>,
}

impl Default for Signin {
    fn default() -> Self {
        Self {
            server_addr: "127.0.0.1:8000".into(),
            name: String::new(),
            error: None,
        }
    }
}

impl Signin {
    pub fn validate(&mut self) -> Option<ValidatedSignin> {
        let server = match self.server_addr.parse::<SocketAddr>() {
            Ok(server) => server,
            Err(err) => {
                self.error = Some(format!("{err}"));

                return None;
            }
        };

        if self.name.is_empty() {
            self.error = Some("Fill in your name".into());

            return None;
        }

        Some(ValidatedSignin {
            server,
            name: self.name.clone(),
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::ServerAddr(new_addr) => self.server_addr = new_addr,
            Message::Name(new_name) => self.name = new_name,
            Message::Error(error) => self.error = Some(error),
            _ => {}
        };

        Task::none()
    }

    pub fn view<'a>(&'a self) -> Element<'a, Message> {
        let mut signin = column![];

        if let Some(error_text) = &self.error {
            signin = signin.push(
                row![
                    // not vibe coded, just lazy emote use instead of proper icons
                    text("🔥").width(FillPortion(1)).align_x(Alignment::End),
                    text(error_text)
                        .width(FillPortion(4))
                        .align_x(Alignment::Start),
                ]
                .align_y(Alignment::Center)
                .spacing(5),
            );
        }

        signin = signin
            .push(
                row![
                    text("Server address:")
                        .width(FillPortion(1))
                        .align_x(Alignment::End),
                    text_input("", &self.server_addr)
                        .on_input(Message::ServerAddr)
                        .width(FillPortion(4))
                ]
                .align_y(Alignment::Center)
                .spacing(5),
            )
            .push(
                row![
                    text("Name:").width(FillPortion(1)).align_x(Alignment::End),
                    row![
                        text_input("", &self.name)
                            .on_input(Message::Name)
                            .on_submit(Message::Signin),
                        button("Sign in").on_press(Message::Signin)
                    ]
                    .spacing(5)
                    .width(FillPortion(4))
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .spacing(5)
            .padding(10);

        container(
            container(signin)
                .width(550.0)
                .height(Length::Shrink)
                .align_y(Alignment::Center)
                .style(container::rounded_box),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
    }
}
