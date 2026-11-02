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
| `build-glassine.sh` | the Flatpak build, in a container; aarch64 runs under qemu, about an hour |

Upstream's GitHub workflows are removed here, so they don't run on this organization's minutes.

**Status:** not what devices run yet. The installed Flatpak is still built elsewhere and uses
the protocol's older name, so this build has no pen preview against a running system today.
It gets one once the compositor uses `glassine_ink_v1` too. On any other compositor it behaves
like Rnote.

Written largely by AI agents and not reviewed by anyone who knows these subsystems well.
