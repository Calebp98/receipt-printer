"""Minimal ESC/POS driver for the Epson TM-T20II over USB (80mm, 42 cols)."""
import sys, textwrap, usb.core, usb.util

VID, PID = 0x04B8, 0x0E15
WIDTH = 42  # chars per line, font A, 80mm paper

ESC, GS, DLE, EOT = b"\x1b", b"\x1d", b"\x10", b"\x04"


class PrinterError(RuntimeError):
    pass


class Printer:
    def __init__(self):
        self.dev = usb.core.find(idVendor=VID, idProduct=PID)
        if self.dev is None:
            raise PrinterError("TM-T20II not found on USB — is it powered on and plugged in?")
        try:
            self.dev.set_configuration()
        except usb.core.USBError as e:
            raise PrinterError(f"could not claim the printer: {e}")
        intf = self.dev.get_active_configuration()[(0, 0)]
        self.out = usb.util.find_descriptor(
            intf, custom_match=lambda e: usb.util.endpoint_direction(e.bEndpointAddress) == usb.util.ENDPOINT_OUT)
        self.inp = usb.util.find_descriptor(
            intf, custom_match=lambda e: usb.util.endpoint_direction(e.bEndpointAddress) == usb.util.ENDPOINT_IN)
        self.buf = bytearray()

    # --- status -----------------------------------------------------------
    def _query(self, n):
        """Send a real-time status request and return the reply byte, or None."""
        self.out.write(DLE + EOT + bytes([n]), timeout=500)
        size = self.inp.wMaxPacketSize
        for _ in range(3):
            data = self.dev.read(self.inp.bEndpointAddress, size, timeout=500)
            if len(data):
                return data[-1]
        return None

    def status(self):
        """Ask the printer how it is. Returns a list of problems, empty if fine."""
        if self.inp is None:
            return []
        problems = []
        try:
            s = self._query(1)                                        # printer status
            if s is not None and s & 0x08:
                problems.append("printer is offline")
            p = self._query(4)                                        # paper sensor
            if p is not None:
                if p & 0x60:
                    problems.append("out of paper")
                elif p & 0x0C:
                    problems.append("paper nearly out")
            o = self._query(2)                                        # offline cause
            if o is not None:
                if o & 0x04:
                    problems.append("cover is open")
                if o & 0x40:
                    problems.append("printer is in an error state")
        except usb.core.USBError:
            pass  # status is best-effort; don't block printing over it
        return problems

    # --- composition ------------------------------------------------------
    def raw(self, b):
        self.buf += b
        return self

    def init(self):
        return self.raw(ESC + b"@")

    def align(self, how):
        return self.raw(ESC + b"a" + bytes([{"left": 0, "centre": 1, "center": 1, "right": 2}[how]]))

    def style(self, bold=False, big=False, tall=False, wide=False, underline=False):
        n = 0
        if bold:
            n |= 0x08
        if tall or big:
            n |= 0x10
        if wide or big:
            n |= 0x20
        if underline:
            n |= 0x80
        return self.raw(ESC + b"!" + bytes([n]))

    def text(self, s, wrap=True):
        for para in str(s).split("\n"):
            lines = textwrap.wrap(para, WIDTH) if (wrap and para) else [para]
            for line in lines or [""]:
                self.buf += line.encode("cp437", "replace") + b"\n"
        return self

    def rule(self, ch="-"):
        return self.text(ch * WIDTH, wrap=False)

    def columns(self, left, right):
        """Left text and right text on one line, dot-padded between."""
        space = WIDTH - len(right)
        left = left[: space - 1]
        return self.text(left + " " * (space - len(left)) + right, wrap=False)

    def qr(self, data, size=6):
        d = data.encode()
        n = len(d) + 3
        self.raw(GS + b"(k" + bytes([4, 0, 49, 65, 50, 0]))            # model 2
        self.raw(GS + b"(k" + bytes([3, 0, 49, 67, size]))             # module size
        self.raw(GS + b"(k" + bytes([3, 0, 49, 69, 48]))               # error correction L
        self.raw(GS + b"(k" + bytes([n & 0xFF, n >> 8, 49, 80, 48]) + d)  # store
        self.raw(GS + b"(k" + bytes([3, 0, 49, 81, 48]))               # print
        return self

    def feed(self, n=1):
        return self.raw(ESC + b"d" + bytes([n]))

    def cut(self, feed=4):
        return self.raw(GS + b"V\x41" + bytes([feed]))

    # --- output -----------------------------------------------------------
    def send(self):
        if not self.buf:
            return 0
        try:
            written = self.out.write(bytes(self.buf), timeout=5000)
        except usb.core.USBTimeoutError:
            problems = self.status()
            raise PrinterError(
                "printer would not accept data: " + (", ".join(problems) if problems
                else "it stopped responding (check the cover is latched and paper is loaded)"))
        self.buf = bytearray()
        return written
