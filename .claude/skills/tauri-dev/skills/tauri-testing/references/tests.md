# Tests

> Source: https://v2.tauri.app/develop/tests/ (tauri-docs `4b74da6`, 2026-10-13). Local copy; site components flattened, links point to the live site.
>
> Techniques for testing inside and outside the Tauri runtime

Tauri offers support for both unit and integration testing utilizing a mock runtime. Under the mock runtime, native
webview libraries are not executed. [See more about the mock runtime here].

Tauri also provides support for end-to-end testing utilizing the WebDriver protocol. [WebdriverIO Tauri testing]
supports Windows, Linux, and macOS; the WebDriver protocol can also be driven directly on Windows and Linux, as macOS
provides no desktop WebDriver client. [See more about WebDriver support here].

We offer [tauri-action] to help run GitHub actions, but any sort of CI/CD runner can be used with Tauri as long as each
platform has the required libraries installed to compile against.

[See more about the mock runtime here]: /develop/tests/mocking/
[See more about WebDriver support here]: /develop/tests/webdriver/
[tauri-action]: https://github.com/tauri-apps/tauri-action
[webdriverio tauri testing]: https://webdriver.io/docs/desktop-testing/tauri
