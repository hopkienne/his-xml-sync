use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager, Runtime,
};

pub fn setup(app: &mut App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Mo lai", true, None::<&str>)?;
    let sync_now = MenuItem::with_id(app, "sync_now", "Dong bo ngay", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Thoat", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &sync_now, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("HIS XML Sync")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Err(error) = show_main_window(app) {
                    eprintln!("failed to show main window: {error}");
                }
            }
            "sync_now" => {
                if let Err(error) = show_main_window(app) {
                    eprintln!("failed to show main window before sync: {error}");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Err(error) = show_main_window(tray.app_handle()) {
                    eprintln!("failed to show main window from tray: {error}");
                }
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        window.set_focus()?;
    }

    Ok(())
}
