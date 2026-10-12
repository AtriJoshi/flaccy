#!/usr/bin/env python3
"""Pushes listing text and screenshots to an editable App Store Connect version.

  publish_listing.py --platform ios|mac --app APP_ID --version-id VERSION_ID --app-info APP_INFO_ID
                     [--stage DIR] [--device-type IPHONE_67] [--skip-text] [--dry-run]

Text comes from metadata_en.json (en-US) and metadata_i18n.json (every other locale); screenshots
from DIR/<ASC locale>/*.png, replacing the whole set for that device type in each locale.
"""
import argparse
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def run(cmd, dry):
    print("+", " ".join(cmd[:6]), "...", flush=True)
    if dry:
        return
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"FAILED: {' '.join(cmd[:8])}\n{result.stdout[-400:]}\n{result.stderr[-400:]}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", required=True, choices=["ios", "mac"])
    parser.add_argument("--app", required=True)
    parser.add_argument("--version-id", required=True)
    parser.add_argument("--app-info", required=True)
    parser.add_argument("--stage", default="")
    parser.add_argument("--device-type", default="IPHONE_67")
    parser.add_argument("--skip-text", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    en = json.load(open(os.path.join(HERE, "metadata_en.json")))[args.platform]
    i18n = json.load(open(os.path.join(HERE, "metadata_i18n.json")))
    texts = {"en-US": en}
    for locale, entry in i18n.items():
        if args.platform in entry:
            texts[locale] = entry[args.platform]

    if not args.skip_text:
        for locale, t in texts.items():
            run(["asc", "localizations", "update", "--version", args.version_id, "--locale", locale,
                 "--description", t["description"], "--keywords", t["keywords"], "--promotional-text", t["promo"]],
                args.dry_run)
            run(["asc", "localizations", "update", "--app", args.app, "--type", "app-info", "--app-info", args.app_info,
                 "--locale", locale, "--subtitle", t["subtitle"]], args.dry_run)

    if args.stage:
        for locale in sorted(os.listdir(args.stage)):
            path = os.path.join(args.stage, locale)
            if not os.path.isdir(path):
                continue
            cmd = ["asc", "screenshots", "upload", "--app", args.app, "--version-id", args.version_id, "--locale", locale,
                   "--path", path, "--device-type", args.device_type, "--replace", "--confirm"]
            run(cmd, args.dry_run)
    print("done", flush=True)


if __name__ == "__main__":
    main()
