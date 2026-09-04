//! breadboard desktop — a thin native shell around https://breadboard.breadway.dev.
//!
//! The window loads the live site directly, so session cookies and the Forgejo
//! OAuth redirect just work. Two global shortcuts:
//!
//!   Ctrl+Shift+B  show / hide the window
//!   Ctrl+Shift+N  show the window and open the "new issue" palette
//!
//! (`#new` in the URL hash is what the SPA watches for.)

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use tauri::{Manager, WebviewWindow};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

fn toggle(win: &WebviewWindow) {
    match win.is_visible() {
        Ok(true) => {
            let _ = win.hide();
        }
        _ => {
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

fn main() {
    let toggle_win = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyB);
    let quick_add = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyN);

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcut(toggle_win)
                .expect("register toggle shortcut")
                .with_shortcut(quick_add)
                .expect("register quick-add shortcut")
                .with_handler(move |app, sc, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let Some(win) = app.get_webview_window("main") else { return };
                    if *sc == toggle_win {
                        toggle(&win);
                    } else if *sc == quick_add {
                        let _ = win.show();
                        let _ = win.set_focus();
                        let _ = win.eval(
                            "location.hash = ''; location.hash = 'new';",
                        );
                    }
                })
                .build(),
        )
        .run(tauri::generate_context!())
        .expect("error while running breadboard-desktop");
}
