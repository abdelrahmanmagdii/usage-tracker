cask "usagebar" do
  version "0.2.6"
  sha256 "319015b8b946b9bb936a50ed64530ef0c7ddd241b767c9fd1db19fb6eb4205d8"

  url "https://github.com/abdelrahmanmagdii/usage-tracker/releases/download/v#{version}/UsageBar_#{version}_universal.dmg"
  name "UsageBar"
  desc "Codex, Claude, Cursor, OpenCode, Devin, Antigravity, and Gemini CLI quotas in the Mac menu bar"
  homepage "https://github.com/abdelrahmanmagdii/usage-tracker"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :catalina"

  app "UsageBar.app"

  zap trash: [
    "~/Library/Application Support/com.abdelrahmanamer.usagebar",
    "~/Library/LaunchAgents/UsageBar.plist",
  ]
end
