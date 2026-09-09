# Audio Visualizer

Audio Visualizer renders the sound currently playing through the default
Windows output device. A supervised `process-v2` companion captures WASAPI
loopback audio; the Canvas layer draws bars, mirrored bars, or a waveform. It
does not record to disk, contact a network service, or expose raw audio outside
the local MyWallpaper native connection.

## Development

Use `mywallpaper dev` for the complete desktop preview. Quality checks build
the web layer and the Windows companion through the reviewed toolchain.

## Publishing

Merge the source and matching manifest/package version into the reviewed default
branch, wait for quality checks, then push a new immutable `v<version>` tag.
Open this add-on's management page in MyWallpaper and select that tag to request
publication with an active lifetime entitlement.

MyWallpaper resolves the exact public repository and commit, dispatches its
pinned central workflow, rebuilds and verifies the artifacts, and publishes the
immutable transport from the platform repository. The add-on repository needs
no publication workflow or MyWallpaper credential. Do not pre-create a GitHub
release: a source tag alone does not publish the add-on to the catalogue.

Each accepted newer release is available for new installations. Existing
wallpapers remain pinned to their exact release until explicitly changed.

## License

MIT. See [LICENSE](LICENSE).
