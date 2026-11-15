# GlassineOS Rnote Ink

Rnote Ink is an unofficial fork of [Rnote](https://github.com/flxzt/rnote) 0.14.2, with its
own name and app ID (`org.glassine.RnoteInk`), so it installs beside upstream Rnote. It adds
a pen preview for the GlassineOS e-ink session, as separate commits on branch
`glassine/0.14.2` on top of upstream's tag. Releases are tagged `<ver>-glassine<N>`.
Upstream's `README.md` still describes Rnote itself, and its GPL-3.0 licence applies.

| Path | What |
|---|---|
| `crates/rnote-ui/src/glassine_ink.rs`, `crates/rnote-ui/src/glassine-ink-v1.xml` | the pen preview: where the canvas is, and the current tool's brush. The protocol file comes from GlassineOS sway |
| `build-aux/org.glassine.RnoteInk.yaml` | the Flatpak manifest |
| `build-glassine.sh` | the Flatpak build, in a container; aarch64 is native on an arm64 host, under qemu (about an hour) on an x86_64 one |
| `.github/workflows/glassine-flatpak.yml`, `build-aux/glassine-release` | CI: the aarch64 bundle on a native arm64 runner; on the default branch a release `<ver>-glassine<N>` (N from `build-aux/glassine-release`) carries it |

Upstream's GitHub workflows are removed here, so they don't run on this organization's minutes.

**Status:** the pen preview speaks `glassine_ink_v1`, the protocol GlassineOS sway offers.
Builds made before the protocol's rename ask for its older name and get no pen preview
from a current compositor; install the bundle from this repository's newest release
(`flatpak install --user --bundle rnote-ink-aarch64.flatpak`). On any other compositor it
behaves like Rnote.

Written largely by AI agents and not reviewed by anyone who knows these subsystems well.
