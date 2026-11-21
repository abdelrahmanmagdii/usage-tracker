# UsageBar Homebrew cask

The cask in this directory mirrors `Casks/usagebar.rb` at the root of
[the repository](https://github.com/abdelrahmanmagdii/usage-tracker), which is
itself the tap. Edit neither by hand for a release: `tools/packages.mjs cask`
pins both to the published `.dmg` digest after each release.

```sh
brew tap abdelrahmanmagdii/usagebar https://github.com/abdelrahmanmagdii/usage-tracker.git
brew install --cask usagebar
brew upgrade --cask usagebar
brew uninstall --cask usagebar
```
