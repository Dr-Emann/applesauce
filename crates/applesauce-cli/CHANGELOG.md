# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.15](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.14...applesauce-cli-v0.5.15) - 2025-07-08

### Other
- *(deps)* Bump the minor-patches group across 1 directory with 4 updates (by @dependabot[bot]) - #150
- Fix clippy warnings on nightly (by @Dr-Emann) - #153
- *(deps)* Bump libc in the minor-patches group (by @dependabot[bot]) - #151
- Fix clippy warnings on nightly (by @Dr-Emann) - #148
- *(deps)* Bump the minor-patches group across 1 directory with 5 updates (by @dependabot[bot]) - #145

## [0.5.14](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.13...applesauce-cli-v0.5.14) - 2025-04-28

### Other
- Fix clippy warnings on nightly (by @Dr-Emann) - #141
- *(deps)* Bump the minor-patches group with 3 updates (by @dependabot[bot]) - #140
- *(deps)* Bump the minor-patches group across 1 directory with 3 updates (by @dependabot[bot]) - #138

## [0.5.13](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.12...applesauce-cli-v0.5.13) - 2025-03-14

### Fixed
- Compile with updated dependencies (by @Dr-Emann) - #131

### Other
- Update Cargo.lock dependencies

## [0.5.12](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.11...applesauce-cli-v0.5.12) - 2025-03-05

### Added
- Enable zlib levels up to 12 (by @Dr-Emann) - #127

### Other
- Use io::Error::other where possible, fix nightly clippy warnings (by @Dr-Emann) - #127

## [0.5.10](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.9...applesauce-cli-v0.5.10) - 2025-02-01

### Fixed
- Avoid re-fetching metadata for files which were not modified (by @Dr-Emann) - #114

## [0.5.9](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.8...applesauce-cli-v0.5.9) - 2025-01-06

### Fixed
- Correctly include skipped files in the final statistics (by @Dr-Emann) - #107

## [0.5.8](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.7...applesauce-cli-v0.5.8) - 2025-01-02

### Fixed
- Ensure cleaned up progress bar before outputting final stats (by @Dr-Emann) - #104

## [0.5.7](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.6...applesauce-cli-v0.5.7) - 2025-01-02

### Fixed
- File writing is canceled for ANY errors (by @Dr-Emann) - #102

## [0.5.6](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.5...applesauce-cli-v0.5.6) - 2025-01-02

### Fixed
- Avoid possible panic when verifying (by @Dr-Emann) - #98

### Other
- Appease clippy nightly (by @Dr-Emann) - #98

## [0.5.5](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.4...applesauce-cli-v0.5.5) - 2024-12-17

### Other

- Bump the minor-patches group across 1 directory with 3 updates ([#78](https://github.com/Dr-Emann/applesauce/pull/78))
- fix clippy warnings

## [0.5.4](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.3...applesauce-cli-v0.5.4) - 2024-07-03

### Other
- Extra post-op stats ([#73](https://github.com/Dr-Emann/applesauce/pull/73))
- Bump the minor-patches group across 1 directory with 3 updates
- Bump the minor-patches group across 1 directory with 3 updates
- Add test to verify things work with hard links present.

## [0.5.3](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.2...applesauce-cli-v0.5.3) - 2024-04-16

### Other
- Only reset directories if we modify the contents
- Save and restore created/added/modified/accessed times
- Only reset a directory's times if it has files under it

## [0.5.2](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.1...applesauce-cli-v0.5.2) - 2024-04-15

### Other
- Add benchmark to compare compressors
- Do not compress a file inside a dir we're also compressing

## [0.5.1](https://github.com/Dr-Emann/applesauce/compare/applesauce-cli-v0.5.0...applesauce-cli-v0.5.1) - 2024-04-15

### Added

- Reset directory modified times as well
