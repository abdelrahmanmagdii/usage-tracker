# Publishing the tap

This directory is the seed for the `abdelrahmanmagdii/homebrew-usagebar` tap
repository. `Casks/usagebar.rb` here is a copy of the canonical cask in the
main repo's `Casks/` directory — edit that one, not this one; the Release
workflow overwrites the tap copy on every tagged release.

Steps:

1. Create a public, empty GitHub repo named `homebrew-usagebar`
   (no README/license — GitHub offers to seed; decline).
2. Copy this directory's contents into it (`README.md`, `Casks/usagebar.rb`),
   commit, push.
3. In `usage-tracker` repo settings set:
   - Variable `HOMEBREW_TAP_REPO` = `abdelrahmanmagdii/homebrew-usagebar`
   - Secret `HOMEBREW_TAP_TOKEN` = a PAT with `repo` scope on the tap repo.

Until those are set the `cask` job in `release.yml` skips silently; once set,
every `v*` tag push syncs the cask into the tap.

Install command once live: `brew install --cask abdelrahmanmagdii/usagebar/usagebar`
(the README install section can then drop its conditional wording).
