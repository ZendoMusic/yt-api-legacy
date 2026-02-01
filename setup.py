import re
import json
import yaml
import requests
import sys
import os
from pathlib import Path
import platform
import shutil

DEFAULT_UA = (
    "Mozilla/5.0 (Web0S; Linux/SmartTV) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/79.0.3945.79 Safari/537.36 "
    "DMOST/2.0.0 (; LGE; webOSTV; WEBOS6.3.2 03.34.95; W6_lm21a;)"
)

print("""
░█░█░█▀█░█░█░▀█▀░█░█░█▀▄░█▀▀░░░█▀█░█▀█░▀█▀░░░█░░░█▀▀░█▀▀░█▀█░█▀▀░█░█░░░█▀▀░█▀▀░▀█▀░█░█░█▀█
░░█░░█░█░█░█░░█░░█░█░█▀▄░█▀▀░░░█▀█░█▀▀░░█░░░░█░░░█▀▀░█░█░█▀█░█░░░░█░░░░▀▀█░█▀▀░░█░░█░█░█▀▀
░░▀░░▀▀▀░▀▀▀░░▀░░▀▀▀░▀▀░░▀▀▀░░░▀░▀░▀░░░▀▀▀░░░▀▀▀░▀▀▀░▀▀▀░▀░▀░▀▀▀░░▀░░░░▀▀▀░▀▀▀░░▀░░▀▀▀░▀░░
""")

# ───────────────────────────────────────────────
# Functions for extracting INNERTUBE_API_KEY (unchanged logic)
# ───────────────────────────────────────────────

def extract_string(key: str, text: str) -> str | None:
    m = re.search(rf'"{re.escape(key)}"\s*:\s*"([^"]+)"', text)
    return m.group(1) if m else None


def extract_innertube_api_key(html: str) -> str | None:
    key = extract_string("INNERTUBE_API_KEY", html)
    if key:
        return key
    m = re.search(r"ytcfg\.set\(\s*({.+?})\s*\)\s*;", html, re.DOTALL)
    if m:
        try:
            cfg = json.loads(m.group(1))
            return cfg.get("INNERTUBE_API_KEY")
        except Exception:
            pass
    return None


def get_fresh_innertube_key(user_agent: str = DEFAULT_UA) -> str | None:
    headers = {
        "User-Agent": user_agent,
        "Accept-Language": "en-US,en;q=0.9",
        "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    }
    urls_to_try = [
        "https://www.youtube.com/tv",
        "https://www.youtube.com/",
        "https://www.youtube.com/embed/",
    ]
    for url in urls_to_try:
        try:
            print(f"Trying to fetch key from → {url}")
            r = requests.get(url, headers=headers, timeout=12)
            if r.status_code != 200:
                continue
            key = extract_innertube_api_key(r.text)
            if key:
                print("Found INNERTUBE_API_KEY:", key)
                return key
        except Exception as e:
            print(f"Error requesting {url}: {e}")
    print("Could not find INNERTUBE_API_KEY on any page.")
    return None


# ───────────────────────────────────────────────
# User input functions
# ───────────────────────────────────────────────

def ask_for_port() -> int:
    while True:
        value = input("Server port [default: 2823]: ").strip()
        if not value:
            return 2823
        try:
            port = int(value)
            if 1 <= port <= 65535:
                return port
            print("Port must be between 1 and 65535. Try again.")
        except ValueError:
            print("Please enter a valid number or press Enter for default.")


def ask_for_main_url() -> str:
    value = input("Main URL (leave empty if not needed) [default: '']: ").strip()
    return value


def ask_for_api_keys() -> list[str]:
    print("\nEnter API keys (comma-separated)")
    print("Examples:")
    print(" AIzaSyABC...,AIzaSyDEF...,AIzaSyGHI...")
    print(" (one key is also fine)")
    print(" (press Enter to use empty list)\n")
    value = input("API keys: ").strip()
    if not value:
        print("→ Using empty list for active keys")
        return []
    keys = [k.strip() for k in value.split(",") if k.strip()]
    if not keys:
        print("→ No valid keys entered → empty list")
        return []
    print(f"→ Added {len(keys)} key(s)")
    return keys


def main():
    print("=== Config.yml Generator ===\n")

    port = ask_for_port()
    main_url = ask_for_main_url()
    active_keys = ask_for_api_keys()
    fresh_key = get_fresh_innertube_key()

    config = {
        "server": {
            "port": port,
            "main_url": main_url,
            "secret_key": "test",
        },
        "api": {
            "request_timeout": 30,
            "keys": {
                "active": active_keys,
                "disabled": [],
            },
            "innertube": {
                "key": fresh_key,
            },
            "oauth": {
                "client_id": "861556708454-d6dlm3lh05idd8npek18k6be8ba3oc68.apps.googleusercontent.com",
                "client_secret": "SboVhoG9s0rNafixCSGGKXAT",
                "redirect_uri": None,
            },
        },
        "video": {
            "source": "innertube",
            "use_cookies": False,
            "default_quality": "360",
            "available_qualities": ["144", "240", "360", "480", "720", "1080", "1440", "2160"],
            "default_count": 50,
        },
        "proxy": {
            "thumbnails": {
                "video": True,
                "channel": False,
                "fetch_channel_thumbnails": False,
            },
            "video_proxy": True,
        },
        "cache": {
            "temp_folder_max_size_mb": 5120,
            "cleanup_threshold_mb": 100,
        },
        "instances": [
            "https://yt.legacyprojects.ru",
            "https://yt.modyleprojects.ru",
            "https://ytcloud.meetlook.ru",
        ],
    }

    output_path = Path("config.yml")
    try:
        with output_path.open("w", encoding="utf-8") as f:
            yaml.dump(config, f, allow_unicode=True, sort_keys=False, indent=2)
        print(f"\nFile successfully created/updated: {output_path.absolute()}")
        print(f"Used INNERTUBE key: {fresh_key}")
        print(f"Server port : {port}")
        print(f"Main URL : '{main_url}'")
        print(f"Active API keys : {len(active_keys)} items")
        if active_keys:
            print(" " + ", ".join(active_keys[:3]) + ("..." if len(active_keys) > 3 else ""))
    except Exception as e:
        print("Error writing file:", e)


if __name__ == "__main__":
    main()