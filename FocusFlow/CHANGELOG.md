# Changelog

## v0.2.0 - Microphone Audio Recording

### Added
- Microphone device listing
- Optional microphone recording
- mic.wav saved inside session folders
- Microphone metadata in metadata.json
- AAC microphone audio muxed into edited.mp4 exports
- Selected microphone persistence

### Improved
- Microphone UI is now part of the normal recording setup
- Mic controls are disabled while recording
- Export detects usable mic.wav before muxing audio
- Invalid or corrupted mic.wav falls back to no-audio export

### Fixed
- Prevent starting mic-enabled recording without a selected microphone
- Avoid marking header-only or unreadable mic.wav as valid audio
- Preserve no-audio export behavior for sessions without microphone audio
