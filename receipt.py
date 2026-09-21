#!/usr/bin/env python3
"""Print text on the receipt printer.

  receipt.py "some text"           print an argument
  echo hi | receipt.py             print stdin
  receipt.py -t "TITLE" file.txt   with a big centred heading
  receipt.py --status              just report how the printer is
"""
import argparse, sys
from escpos import Printer, PrinterError


def main():
    ap = argparse.ArgumentParser(description="Print text on the Epson TM-T20II.")
    ap.add_argument("text", nargs="*", help="text to print (default: read stdin)")
    ap.add_argument("-t", "--title", help="big centred heading")
    ap.add_argument("-f", "--file", help="read text from a file")
    ap.add_argument("-q", "--qr", help="append a QR code with this content")
    ap.add_argument("--no-cut", action="store_true", help="leave the paper uncut")
    ap.add_argument("--status", action="store_true", help="report printer status and exit")
    args = ap.parse_args()

    try:
        p = Printer()
    except PrinterError as e:
        sys.exit(f"receipt: {e}")

    if args.status:
        problems = p.status()
        print("; ".join(problems) if problems else "ready")
        return

    if args.file:
        body = open(args.file).read()
    elif args.text:
        body = " ".join(args.text)
    elif not sys.stdin.isatty():
        body = sys.stdin.read()
    else:
        ap.error("nothing to print — pass text, use --file, or pipe stdin")

    p.init()
    if args.title:
        p.align("centre").style(big=True, bold=True).text(args.title)
        p.style().text("").align("left")
    p.text(body.rstrip("\n"))
    if args.qr:
        p.feed(1).align("centre").qr(args.qr).align("left")
    p.feed(2)
    if not args.no_cut:
        p.cut()

    try:
        p.send()
    except PrinterError as e:
        sys.exit(f"receipt: {e}")


if __name__ == "__main__":
    main()
