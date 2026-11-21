# Tap setup

No separate repository, token or variable is needed. The main repository is the
tap, and the `cask` job in `.github/workflows/release.yml` commits the pinned
digest to its default branch after a release is public. If branch protection
blocks that push, run `node tools/packages.mjs cask <version> <dmg>` locally
and commit both cask files.
