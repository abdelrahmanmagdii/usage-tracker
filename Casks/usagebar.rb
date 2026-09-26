cask "usagebar" do
  version "0.1.4"
  sha256 :no_check

  url "https://github.com/abdelrahmanmagdii/usage-tracker/releases/download/v#{version}/UsageBar_#{version}_universal.dmg"
  name "UsageBar"
  desc "Codex, Claude, Cursor, OpenCode, Devin, and Antigravity quotas in the Mac menu bar"
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
