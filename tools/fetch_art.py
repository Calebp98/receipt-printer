#!/usr/bin/env python3
"""Collect small ASCII art from asciiart.eu into art/art.json.

Only keeps pieces that fit the receipt printer: narrow enough for 48 columns
(or 24 when printed double-size) and short enough to sit on a slip.
"""
import html, json, re, sys, time, urllib.request

BASE = "https://www.asciiart.eu"
CATEGORIES = [
    "animals", "food-and-drinks", "space", "holiday-and-events", "nature",
    "computers", "music", "plants", "toys", "sports-and-outdoors",
    "miscellaneous", "mythology", "electronics", "art-and-design", "cartoons",
]
MAX_WIDTH = 44      # leaves a margin inside the 48-column line
MAX_HEIGHT = 16
CARD = re.compile(
    r'data-title="(?P<title>[^"]*)"\s+data-artist="(?P<artist>[^"]*)"\s+'
    r'data-height="(?P<height>\d+)"\s+data-width="(?P<width>\d+)".*?'
    r'<div class="art-card__ascii">(?P<art>.*?)</div>',
    re.S,
)
# cp437 box drawing and shades survive the trip; anything else does not
EXTRA_OK = set("─═█▄▀░▒▓│┌┐└┘├┤┬┴┼║╔╗╚╝")


def get(url):
    req = urllib.request.Request(url, headers={"User-Agent": "receipt-printer/0.1"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return r.read().decode("utf-8", "replace")


def printable(art):
    return all(c == "\n" or (32 <= ord(c) < 127) or c in EXTRA_OK for c in art)


def subcategories(category):
    page = get(f"{BASE}/{category}")
    found = re.findall(rf'href="(/{re.escape(category)}/[a-z0-9-]+)"', page)
    return sorted(set(found)) or [f"/{category}"]


def scrape(path):
    page = get(BASE + path)
    out = []
    for m in CARD.finditer(page):
        art = html.unescape(m.group("art")).replace("\r\n", "\n").rstrip("\n")
        w, h = int(m.group("width")), int(m.group("height"))
        if w > MAX_WIDTH or h > MAX_HEIGHT or h < 2 or not printable(art):
            continue
        out.append({
            "title": html.unescape(m.group("title")),
            "artist": html.unescape(m.group("artist")),
            "width": w,
            "height": h,
            "source": BASE + path,
            "art": art,
        })
    return out


def main():
    categories = sys.argv[1:] or CATEGORIES
    seen, collected = set(), []
    for category in categories:
        for path in subcategories(category):
            try:
                pieces = scrape(path)
            except Exception as e:
                print(f"  {path}: {e}", file=sys.stderr)
                continue
            fresh = [p for p in pieces if p["art"] not in seen]
            seen.update(p["art"] for p in fresh)
            collected.extend(fresh)
            print(f"  {path}: {len(fresh)}")
            time.sleep(0.4)  # be polite
    collected.sort(key=lambda p: (p["title"].lower(), p["width"]))
    with open("art/art.json", "w") as f:
        json.dump(collected, f, indent=1)
    print(f"{len(collected)} pieces -> art/art.json")


if __name__ == "__main__":
    main()
