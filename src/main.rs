mod app;
mod completion;
mod connector;
mod design;
mod diagram;
mod editing;
mod export;
mod icon;
mod import;
mod manager;
mod plugins;
mod store;
mod theme;
mod tunnel;

use app::App;

fn main() -> iced::Result {
    // Au démarrage : l'état, puis une vérification de mise à jour en arrière-plan.
    let boot = || {
        let app = App::new();
        (app, iced::Task::perform(plugins::check_app_update(), app::Message::UpdateChecked))
    };
    iced::application(boot, App::update, App::view)
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
