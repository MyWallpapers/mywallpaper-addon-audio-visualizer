# Audio Visualizer

Audio Visualizer renders the sound currently playing through the default
Windows output device. A supervised `process-v2` companion captures WASAPI
loopback audio; the Canvas layer draws bars, mirrored bars, or a waveform. It
does not record to disk, contact a network service, or expose raw audio outside
the local MyWallpaper native connection.

## Development

Use `mywallpaper dev` for the complete desktop preview. Quality checks build
the web layer and both Windows companion architectures through the exact
reviewed toolchain.

## Publishing

The immutable OIDC admission workflow performs two independent rebuilds before
MyWallpaper accepts a release. Native execution still requires explicit user
consent for the exact add-on version and digest.

## License

MIT. See [LICENSE](LICENSE).
