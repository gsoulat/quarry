mod app;
mod connector;
mod design;
mod diagram;
mod editing;
mod icon;
mod manager;
mod store;
mod theme;

use app::App;

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(1280.0, 800.0),
            icon: icon::window_icon(),
            ..Default::default()
        })
        .run()
}
