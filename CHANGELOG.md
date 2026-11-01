# Changelog

Notable changes to this fork, on top of upstream
(`jjnuthuagen/cosmic-start-menu-applet`, at the repo-publishing commit).

## Unreleased

### Fixed

- A tile GIF cannot take the applet down: the decoder is given a canvas and
  allocation ceiling, instead of `image`'s unlimited default (a header
  claiming 65535x65535 is 17 GB allocated before a frame is read).
- The menu and the Settings window no longer share one temp file when they
  save `config.toml`, so two saves at once cannot leave a half-written config
  in place.
- Recovering from an unparseable `config.toml` never overwrites the file when
  it could not first be moved aside, and Settings refuses to save over it
  rather than replacing the pins it could not read with an empty menu.
- An App Library folder filter this build does not know is logged at warn
  instead of debug, since the Folders view is then empty.
- Search matches a query word by word, so an app whose name holds every word —
  `office libre`, `fire fox` — is found instead of returning nothing.
- A tile colour with a non-ASCII character where 3 or 6 hex digits are expected
  no longer panics the applet at render.
- An animated GIF keeps its shape: frames cover-crop the tile like a still,
  instead of being stretched to the tile box.
- A tile picture is sniffed from six bytes rather than reading the whole file
  (a video, a huge photo) on every frame, and GIF frames decode one at a time,
  capped.
- A folder no longer repeats an app that the pinned "Most used" block already
  shows — it was drawn twice and was two keyboard stops.
- The Folders label is measured with a `Line::Label`'s height, so a letter jump
  in Folders mode lands on its header instead of 6 px past it.

### Changed

- `just restart` cycles whichever panel's wing lists the applet, so an install
  into the Dock is restarted rather than killed and left on the old build.
- CI runs the build on manual dispatch as well as on a push.
