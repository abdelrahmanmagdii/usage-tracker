// The Windows build reuses the macOS copy with the tray and the PC swapped in.
export const isWindows =
  typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);

export const trayName = isWindows ? "system tray" : "menu bar";
export const deviceName = isWindows ? "PC" : "Mac";
export const notificationSettings = isWindows
  ? "Settings › System › Notifications"
  : "System Settings › Notifications";
