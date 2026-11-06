# breadboard for Android

A thin WebView shell around `https://breadboard.breadway.dev`. It contains no
business logic: the SPA already handles the board, issues, agents, chat
(including image/file attachments), filters and keyboard shortcuts.

## What the shell adds

- Persistent Forgejo session in the app's cookie jar.
- File picker wired into the chat attach button, including multi-select.
- Downloads use the session cookie (via `DownloadManager`).
- OAuth redirects through `git.breadway.dev` stay in the WebView, so login
  completes and the session cookie comes back.
- Back follows web history before leaving the app.
- External links open in the real browser.
- Offline/error screen with retry.

## Build

Requires Android SDK 36, build-tools 35+, and JDK 17. Gradle is provided by the
wrapper.

```sh
export JAVA_HOME=/usr/lib/jvm/java-17-openjdk
export ANDROID_HOME=$HOME/Android
./gradlew assembleDebug
# app/build/outputs/apk/debug/app-debug.apk
```

`local.properties` needs `sdk.dir` pointing at the SDK; it is gitignored.

## Install

```sh
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

Wireless debugging:

1. On the phone: Developer options -> Wireless debugging -> Pair device with
   pairing code.
2. `adb pair <ip>:<pairing-port> <code>`
3. `adb connect <ip>:<connect-port>` (the port on the Wireless debugging
   screen, not the pairing dialog)
4. `adb install -r app/build/outputs/apk/debug/app-debug.apk`

## Limitations

- It is the live site: no offline board cache.
- No camera capture path; uploads go through the system document picker.
- WebView does not support the Web Notifications API, so chat notifications
  appear in the in-app UI only.
- `HOME` is a constant in `MainActivity.java`; change it to point at another
  breadboard instance.
