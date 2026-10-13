mod app;
mod connector;
mod theme;

use app::App;

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .window_size((1280.0, 800.0))
        .run()
}
