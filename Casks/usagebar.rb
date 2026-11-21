cask "usagebar" do
  version "0.2.5"
  sha256 "ffa8f8796f118922480a5753908376e2da67f34a996e065e03a14dfcbaf7ace1"

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
