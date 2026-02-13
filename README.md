<p>
  <img src="assets/logo.png" alt="Logo" width="25%">
</p>

# YouTube API Legacy

[![Rust](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

This is a legacy YouTube API service that provides endpoints for fetching video information, channel data, and more.<br>
Part of the LegacyProjects initiative, bringing back the classic YouTube experience.<br><br>
This is a remaster of [this repository](https://github.com/zemonkamin/ytapilegacy).


## Installation

### Building from source
You will need [Rust](https://rust-lang.org/tools/install/) installed on your system.

1. Clone this repository with `git clone https://github.com/ZendoMusic/yt-api-legacy`
2. From the project folder, run `cargo build --release`
3. If you don't have errors, your file will be compiled in `target/release`

### Deploying
1. Download the latest version from the [releases page](https://github.com/ZendoMusic/yt-api-legacy/releases/) or the [GitHub Actions page](https://github.com/ZendoMusic/yt-api-legacy/actions).
2. Go to the `assets` folder (create it if it doesn’t exist) and download the latest **yt-dlp** binary for your system from the [official releases page](https://github.com/yt-dlp/yt-dlp/releases/).
3. Remove `.example` from the `config.yml.example` file. If you don't have this file, download it from the repository.
4. Open `config.yml` and edit it to suit your needs.
5. Run the binary and enjoy.
