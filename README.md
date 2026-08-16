# Hi from Claude!

<p align="center">
  <img src="hello.svg" alt="Hi from Claude" width="400"/>
</p>

Welcome to the playground repository.

<p align="center">
  <a href="https://www.djangoproject.com/">
    <img src="frameworks.svg" alt="Frontend and Backend Frameworks" width="720"/>
  </a>
</p>

See [LICENSES_LOGOS.md](LICENSES_LOGOS.md) for logo attribution and licensing details.

## Patch drop-box

This repo doubles as a drop-box for changes targeting repos that cloud
sessions can't push to: a session clones a public repo, commits changes,
and exports them here as a `git format-patch` series that you apply locally
with `git am`. See [patches/README.md](patches/README.md) for the convention
and the `scripts/new-patch.sh` / `scripts/apply-patch.sh` helpers.
