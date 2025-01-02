# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/Dr-Emann/applesauce/compare/applesauce-v0.5.5...applesauce-v0.6.0) - 2025-01-02

### Fixed
- Avoid possible panic when verifying (by @Dr-Emann) - #98

## [0.5.5](https://github.com/Dr-Emann/applesauce/compare/applesauce-v0.5.4...applesauce-v0.5.5) - 2024-12-17

### Other

- fix clippy warnings

## [0.5.4](https://github.com/Dr-Emann/applesauce/compare/applesauce-v0.5.3...applesauce-v0.5.4) - 2024-07-03

### Other
- Bump the minor-patches group across 1 directory with 3 updates
- Bump the minor-patches group across 1 directory with 3 updates
- Add test to verify things work with hard links present.

## [0.5.3](https://github.com/Dr-Emann/applesauce/compare/applesauce-v0.5.2...applesauce-v0.5.3) - 2024-04-16

### Other
- Only reset directories if we modify the contents
- Save and restore created/added/modified/accessed times
- Only reset a directory's times if it has files under it

## [0.5.2](https://github.com/Dr-Emann/applesauce/compare/applesauce-v0.5.1...applesauce-v0.5.2) - 2024-04-15

### Other
- Add benchmark to compare compressors
- Do not compress a file inside a dir we're also compressing

## [0.5.1](https://github.com/Dr-Emann/applesauce/compare/applesauce-v0.5.0...applesauce-v0.5.1) - 2024-04-15

### Added

- Reset directory modified times as well
