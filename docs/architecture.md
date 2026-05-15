# Widgex Architecture

Widgex is a desktop widget runtime that starts on Arch Linux and keeps a cross-platform boundary from the beginning.

The Rust daemon owns configuration, data sources, permissions, platform integration, and window lifecycle. The renderer receives a normalized widget tree and sends user actions back to the daemon.
