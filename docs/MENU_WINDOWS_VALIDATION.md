# Windows menu and taskbar validation

Automated geometry and state tests cover popup work-area clamping, negative monitor coordinates, 100–200% DPI scaling, page Back targets, checked menu rows, provider selection, settings round trips, and notification-area bounds. The Win32 popup still needs interactive validation on Windows 10 and 11.

## Manual checklist

- [ ] Cold start with Explorer already running; check widget placement, tray icon, and menu startup latency.
- [ ] Repeated launch; confirm only one instance and no duplicate tray icons or menu HWNDs.
- [ ] Open from the widget and from each provider tray icon; verify the popup has no taskbar button.
- [ ] Open and close repeatedly; check idle CPU, private bytes, GDI object count, and handle count for growth.
- [ ] Click outside the popup and press Escape; verify immediate dismissal. Open each page and use Back.
- [ ] Navigate with Tab, Shift+Tab, arrows, Enter, Space, and Escape. Verify the focus ring and screen-reader names, including selected rows.
- [ ] Toggle every provider; confirm that the last enabled provider cannot be disabled and that tray badges and widget width update.
- [ ] Select each refresh interval, language, and theme; restart and confirm persistence. In System theme, switch Windows light/dark while the app is running.
- [ ] Test primary and secondary monitors with negative coordinates, taskbars on each edge, and 100%, 125%, 150%, 175%, and 200% DPI. Open near every screen boundary and scroll the Language page on a short work area.
- [ ] Move or resize the taskbar, show/hide system tray icons, and change enabled provider count. Check that the widget never enters `TrayNotifyWnd`.
- [ ] Restart Explorer while the popup is open and while the widget is hidden; confirm recovery and tray-icon registration.
- [ ] With RU, UK, and EN input languages installed, capture `scripts/check-input-languages.ps1` output before launch, after launch, and if UK disappears. Check both Win+Space in a text field and the taskbar switcher. Use `--diagnose` for the matching taskbar and input-state log; see `docs/INPUT_LANGUAGE_DIAGNOSTICS_RU.md`.

The input-language issue is intermittent. A successful launch or a clean static test does not prove it is resolved.
