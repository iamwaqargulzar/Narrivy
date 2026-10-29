#!/usr/bin/python3
"""Keep pictures of what is shown on screen during a meeting (Narrivy, D34).

Run by narrivyd with the system Python (it needs PyGObject and GStreamer
with the PipeWire plugin, `gstreamer1.0-pipewire`). The screen-sharing
portal asks the user which screen or window to share; nothing is captured
without that choice, and the portal shows that sharing is on.

The shared screen is looked at every couple of seconds. A picture is kept
only when the screen has changed from the last kept one and then stayed
the same for a moment (a new slide, a new page), or, for something that
keeps moving (a video), at most every 30 s. Pictures are written to the
folder given:

  frame-0001.png   the screen, 1024 px wide (for the model)
  thumb-0001.png   480 px wide (for the app)
  frames.jsonl     {"file", "thumb", "at_ms", "source"} per picture, ms since start
  sources.json     the shared screens and windows: kind, width, height
  live-0.png       what each one shows now, small, every second
  now-0.png        the same, 1024 px wide, every few seconds, for the
                   assistant during a live share

With several screens or windows chosen, each is watched on its own.
With --once, one picture of the shared screen is kept, and it stops.
NARRIVY_SCREEN_RESTORE: a restore token from an earlier run in the same
recording, so switching the screen back on does not ask again.

stdout: JSON lines for narrivyd ({"ev": "ready"}, {"ev": "frame", "n": 3},
{"ev": "error", "code": ...}). Stops on SIGTERM or when its parent ends.
"""

import argparse
import json
import os
import signal
import sys
import threading
import time
from pathlib import Path

import gi

gi.require_version("Gst", "1.0")
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import Gio, GLib, GdkPixbuf, Gst  # noqa: E402

PORTAL = "org.freedesktop.portal.Desktop"
PATH = "/org/freedesktop/portal/desktop"
CAST = "org.freedesktop.portal.ScreenCast"
REQUEST = "org.freedesktop.portal.Request"
WIDTH = 1024
THUMB = 480
# The live picture of each shared source, so the app shows what is watched:
# small and often, so it follows the screen. The large one is for the
# assistant and is written less often.
LIVE = 320
LIVE_EVERY = 1.0
NOW_EVERY = 3.0
# A sample counts as changed above this difference (0..255), and the screen
# as changed when this share of samples did.
PIXEL_DIFF = 28
CHANGED_SHARE = 0.03
MOVING_KEEP_S = 30
# The desktop's screen-sharing service ending mid-share (COSMIC's crashes
# now and then): sharing is started again this many times at most.
RESUMES = 5
BACKENDS = "org.freedesktop.impl.portal.desktop."


def emit(**event):
    sys.stdout.write(json.dumps(event) + "\n")
    sys.stdout.flush()


class Portal:
    """The screen-sharing portal: a session, the user's choice, a stream."""

    def __init__(self):
        # Its own loop: portal calls can be made while the main one runs.
        self.loop = GLib.MainLoop()
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION)
        self.sender = self.bus.get_unique_name()[1:].replace(".", "_")
        self.counter = 0
        self.session = None

    def call(self, method, *args):
        """Call a portal method that answers with a Request's Response."""
        self.counter += 1
        token = f"narrivy{os.getpid()}_{self.counter}"
        path = f"/org/freedesktop/portal/desktop/request/{self.sender}/{token}"
        answer = {}

        def on_response(_conn, _sender, _path, _iface, _signal, params):
            code, results = params.unpack()
            answer["code"], answer["results"] = code, results
            self.loop.quit()

        sub = self.bus.signal_subscribe(PORTAL, REQUEST, "Response", path, None, 0, on_response)
        signature, values = args[0], list(args[1:])
        values[-1] = dict(values[-1], handle_token=GLib.Variant("s", token))
        self.bus.call_sync(PORTAL, PATH, CAST, method, GLib.Variant(signature, tuple(values)),
                           None, 0, -1, None)
        self.loop.run()
        self.bus.signal_unsubscribe(sub)
        return answer.get("code", 2), answer.get("results", {})

    def close(self):
        """End the screen-cast session: the portal stops the stream and the
        desktop stops showing that the screen is shared. Ending the process
        alone leaves that to the portal noticing, which COSMIC's does not
        always do."""
        if self.session is None:
            return
        try:
            self.bus.call_sync(PORTAL, self.session, "org.freedesktop.portal.Session", "Close",
                               None, None, 0, 2000, None)
        except GLib.Error:
            pass
        self.session = None

    def cursor_modes(self):
        try:
            reply = self.bus.call_sync(
                PORTAL, PATH, "org.freedesktop.DBus.Properties", "Get",
                GLib.Variant("(ss)", (CAST, "AvailableCursorModes")), None, 0, 2000, None,
            )
            return int(reply.unpack()[0])
        except GLib.Error:
            return 0

    def start(self, multiple=True, restore=None, keep=False):
        """(fd, [(node, properties)]) of the streams the user chose, or an
        error code."""
        code, results = self.call(
            "CreateSession", "(a{sv})",
            {"session_handle_token": GLib.Variant("s", f"narrivys{os.getpid()}")},
        )
        if code != 0:
            return None, "screen_portal_failed"
        session = self.session = results["session_handle"]
        options = {
            "types": GLib.Variant("u", 1 | 2),  # a monitor or a window
            "multiple": GLib.Variant("b", multiple),
        }
        # The pointer drawn into the picture: some compositors send a
        # picture only when the screen changes, and moving the pointer then
        # counts.
        if self.cursor_modes() & 2:
            options["cursor_mode"] = GLib.Variant("u", 2)
        # Within one recording the screen can be switched off and on again:
        # the portal hands back a token for the same choice (held by
        # narrivyd in memory only for that recording).
        if keep:
            options["persist_mode"] = GLib.Variant("u", 2)
            if restore:
                options["restore_token"] = GLib.Variant("s", restore)
        code, _ = self.call("SelectSources", "(oa{sv})", session, options)
        if code != 0:
            return None, "screen_portal_failed"
        code, results = self.call("Start", "(osa{sv})", session, "", {})
        if code == 1:
            return None, "screen_permission_denied"
        if code != 0 or not results.get("streams"):
            return None, "screen_portal_failed"
        streams = [(node, dict(props)) for node, props in results["streams"]]
        reply, fds = self.bus.call_with_unix_fd_list_sync(
            PORTAL, PATH, CAST, "OpenPipeWireRemote",
            GLib.Variant("(oa{sv})", (session, {})), None, 0, -1, None, None,
        )
        fd = fds.get(reply.unpack()[0])
        self.restore_token = results.get("restore_token") or restore
        return (fd, streams), None

    def on_lost(self, callback):
        """Call `callback` when the desktop's screen-sharing service ends
        while sharing (COSMIC's crashes now and then): the stream then just
        stops, without an error. Not when the user stops sharing from the
        desktop: that service stays."""
        def owner_changed(_conn, _sender, _path, _iface, _signal, params):
            name, old, new = params.unpack()
            if self.session is not None and name.startswith(BACKENDS) and old and not new:
                callback()

        self.bus.signal_subscribe("org.freedesktop.DBus", "org.freedesktop.DBus", "NameOwnerChanged",
                                  "/org/freedesktop/DBus", None, 0, owner_changed)


def samples(data, width, height, stride, grid=(64, 36)):
    """A small grid of luminance samples of an RGB frame."""
    out = []
    gx, gy = grid
    for j in range(gy):
        row = (j * height // gy) * stride
        for i in range(gx):
            k = row + (i * width // gx) * 3
            out.append((data[k] * 3 + data[k + 1] * 6 + data[k + 2]) // 10)
    return out


def changed(a, b):
    if a is None or b is None:
        return True
    differ = sum(1 for x, y in zip(a, b) if abs(x - y) > PIXEL_DIFF)
    return differ > CHANGED_SHARE * len(a)


def scaled(size):
    """The caps that shrink a source of `size` to WIDTH. Both sides are
    given: some compositors announce a nonsense pixel shape
    (1/2147483647), and working out the height from it overflows."""
    width, height = size
    if width <= 0 or height <= 0:
        return "video/x-raw,format=RGB"
    if width <= WIDTH:
        return f"videoscale add-borders=false ! video/x-raw,format=RGB,width={width},height={height}"
    tall = max(2, round(WIDTH * height / width / 2) * 2)
    return f"videoscale add-borders=false ! video/x-raw,format=RGB,width={WIDTH},height={tall}"


def pixbuf_of(data, width, height, stride):
    return GdkPixbuf.Pixbuf.new_from_bytes(
        GLib.Bytes.new(data), GdkPixbuf.Colorspace.RGB, False, 8, width, height, stride
    )


def save(folder, n, pixbuf, at_ms, source):
    name, thumb = f"frame-{n:04d}.png", f"thumb-{n:04d}.png"
    pixbuf.savev(str(folder / name), "png", [], [])
    width, height = pixbuf.get_width(), pixbuf.get_height()
    small = pixbuf.scale_simple(THUMB, max(1, height * THUMB // width), GdkPixbuf.InterpType.BILINEAR)
    small.savev(str(folder / thumb), "png", [], [])
    with open(folder / "frames.jsonl", "a") as out:
        out.write(json.dumps({"file": name, "thumb": thumb, "at_ms": at_ms, "source": source}) + "\n")


def save_live(folder, pixbuf, source, large):
    """What source `source` shows now: small for the app (live-N.png), and,
    with `large`, as large as kept pictures for the assistant to look at
    (now-N.png)."""
    width, height = pixbuf.get_width(), pixbuf.get_height()
    small = pixbuf.scale_simple(LIVE, max(1, height * LIVE // width), GdkPixbuf.InterpType.BILINEAR)
    part = folder / f"live-{source}.part"
    small.savev(str(part), "png", [], [])
    os.replace(part, folder / f"live-{source}.png")
    if not large:
        return
    part = folder / f"now-{source}.part"
    pixbuf.savev(str(part), "png", [], [])
    os.replace(part, folder / f"now-{source}.png")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("folder", type=Path)
    parser.add_argument("--interval", type=float, default=2.0)
    parser.add_argument("--start-ms", type=int, default=0, help="the meeting's start, Unix ms")
    parser.add_argument("--moving-every", type=float, default=MOVING_KEEP_S,
                        help="keep a picture of something moving at most this often (s)")
    parser.add_argument("--once", action="store_true",
                        help="keep one picture of the screen the user shares, then stop")
    args = parser.parse_args()
    os.umask(0o077)
    args.folder.mkdir(parents=True, exist_ok=True)
    os.chmod(args.folder, 0o700)
    started_ms = args.start_ms or int(time.time() * 1000)

    Gst.init(None)
    if Gst.ElementFactory.find("pipewiresrc") is None:
        emit(ev="error", code="screen_capture_unavailable")
        return 2
    loop = GLib.MainLoop()
    # Kept for the whole run: the screen-sharing session lives as long as
    # this D-Bus connection, and the stream stops when it closes.
    portal = Portal()
    chosen, error = portal.start(
        multiple=not args.once,
        restore=os.environ.get("NARRIVY_SCREEN_RESTORE"),
        keep=not args.once,
    )
    if error:
        portal.close()
        emit(ev="error", code=error)
        return 1
    fd, streams = chosen
    # The stream's PipeWire connection: replaced when sharing starts again.
    remote = {"fd": fd}
    # Pictures arrive as often as the live picture needs; changes are
    # looked for every interval.
    rate = max(1, round(min(args.interval, LIVE_EVERY)))
    # Switched back on in the same recording: numbering goes on.
    try:
        with open(args.folder / "frames.jsonl") as kept:
            first = sum(1 for _ in kept)
    except OSError:
        first = 0
    counter = {"n": first}
    lock = threading.Lock()
    sources = []
    watched = []
    trouble = {"restarts": 0}

    def look(state, source, data, width, height):
        """Compare a picture of `source` with the last kept one; keep it
        once a change has settled. Runs on the main loop."""
        stride = len(data) // height
        now = time.monotonic()
        if args.once:
            if counter["n"] == 0:
                counter["n"] = 1
                save(args.folder, 1, pixbuf_of(data, width, height, stride), 0, source)
                loop.quit()
            return
        # A little early still counts: pictures come about once a second.
        if now - state["live"] >= LIVE_EVERY * 0.8:
            state["live"] = now
            large = now - state["now"] >= NOW_EVERY * 0.9
            if large:
                state["now"] = now
            save_live(args.folder, pixbuf_of(data, width, height, stride), source, large)
        if now - state["looked"] < args.interval * 0.9:
            return
        state["looked"] = now
        grid = samples(data, width, height, stride)
        keep = False
        if changed(state["kept"], grid):
            # Keep a new screen once it has settled; something that keeps
            # moving is kept now and then.
            if state["candidate"] is not None and not changed(state["candidate"], grid):
                keep = True
            elif now - state["since_kept"] > args.moving_every:
                keep = True
            state["candidate"] = grid
        else:
            state["candidate"] = None
        if keep:
            state["kept"], state["candidate"], state["since_kept"] = grid, None, now
            with lock:
                counter["n"] += 1
                n = counter["n"]
            at_ms = int(time.time() * 1000) - started_ms
            save(args.folder, n, pixbuf_of(data, width, height, stride), at_ms, source)
            emit(ev="frame", n=n)

    def connect(watch):
        """(Re)connect one source's stream. Some compositors (COSMIC) drop
        the stream's format once it starts and then send pictures only when
        the screen changes; a fresh connection always brings one."""
        # State changes run off the main loop: a live source can wait a long
        # time for the compositor's first picture.
        if watch.get("pipeline") is not None:
            watch["pipeline"].call_async(lambda element, _data: element.set_state(Gst.State.NULL), None)
        pipeline = Gst.parse_launch(
            f"pipewiresrc fd={os.dup(remote['fd'])} path={watch['node']} do-timestamp=true always-copy=true "
            f"! videorate drop-only=true ! video/x-raw,framerate=1/{rate} "
            f"! videoconvert ! {scaled(watch['size'])} "
            "! appsink name=frames emit-signals=true max-buffers=1 drop=true sync=false"
        )
        generation = watch["generation"] = watch.get("generation", 0) + 1

        def on_sample(appsink):
            # The streaming thread only copies the picture: holding it up
            # makes the compositor drop the stream.
            sample = appsink.emit("pull-sample")
            caps = sample.get_caps().get_structure(0)
            width, height = caps.get_value("width"), caps.get_value("height")
            buffer = sample.get_buffer()
            ok, info = buffer.map(Gst.MapFlags.READ)
            if not ok:
                return Gst.FlowReturn.OK
            try:
                data = bytes(info.data)
            finally:
                buffer.unmap(info)

            def arrived():
                watch["latest"] = (data, width, height)
                watch["at"] = time.monotonic()
                trouble["restarts"] = 0
                look(watch["state"], watch["source"], data, width, height)
                return False

            GLib.idle_add(arrived)
            return Gst.FlowReturn.OK

        pipeline.get_by_name("frames").connect("new-sample", on_sample)
        bus = pipeline.get_bus()
        bus.add_signal_watch()

        def on_message(_bus, message):
            if message.type != Gst.MessageType.ERROR or watch["generation"] != generation:
                return
            # GStreamer's own words go to stderr (no picture content).
            error, debug = message.parse_error()
            print(f"screen_capture: {error.message} ({debug})", file=sys.stderr, flush=True)
            GLib.timeout_add(500, lambda: restart(watch) and False)

        bus.connect("message", on_message)
        watch["pipeline"] = pipeline
        watch["connected"] = time.monotonic()
        pipeline.call_async(lambda element, _data: element.set_state(Gst.State.PLAYING), None)

    def restart(watch):
        trouble["restarts"] += 1
        if trouble["restarts"] > 30:
            emit(ev="error", code="screen_stream_failed")
            loop.quit()
            return False
        connect(watch)
        return True

    def tend():
        """Every interval, a source whose screen did not change is looked at
        again: compositors send pictures only when something on screen
        changes, and the same picture again lets a change settle."""
        now = time.monotonic()
        for watch in watched:
            if watch["latest"] is not None and now - watch["at"] >= args.interval:
                data, width, height = watch["latest"]
                look(watch["state"], watch["source"], data, width, height)
        return True

    def watch_streams(streams):
        sources.clear()
        watched.clear()
        for index, (node, props) in enumerate(streams):
            size = props.get("size", (0, 0))
            kind = {1: "monitor", 2: "window"}.get(props.get("source_type", 0), "screen")
            sources.append({"source": index, "kind": kind, "width": size[0], "height": size[1]})
            watched.append({
                "source": index, "node": node, "size": size, "latest": None, "at": 0.0,
                "state": {"kept": None, "candidate": None, "since_kept": time.monotonic(), "live": 0.0,
                          "now": 0.0, "looked": 0.0},
            })
        with open(args.folder / "sources.json", "w") as out:
            json.dump(sources, out)

    resumed = {"count": 0, "pending": False}

    def resume():
        """Sharing ended under us: start it again with the same choice (the
        portal may ask the user again when it cannot restore it)."""
        resumed["pending"] = False
        if resumed["count"] >= RESUMES:
            emit(ev="error", code="screen_stream_failed")
            loop.quit()
            return False
        resumed["count"] += 1
        print("screen_capture: sharing ended by the desktop; starting it again", file=sys.stderr, flush=True)
        for watch in watched:
            if watch.get("pipeline") is not None:
                watch["generation"] = -1
                watch["pipeline"].call_async(lambda element, _data: element.set_state(Gst.State.NULL), None)
        portal.session = None
        old = remote["fd"]
        again, error = portal.start(multiple=True, restore=portal.restore_token, keep=True)
        if error:
            emit(ev="error", code=error)
            loop.quit()
            return False
        remote["fd"], streams = again
        os.close(old)
        watch_streams(streams)
        for watch in watched:
            connect(watch)
        emit(ev="ready", sources=sources, restore_token=portal.restore_token)
        return False

    def lost():
        # Once for a burst of signals; a moment for the service to come back.
        if not resumed["pending"] and not args.once:
            resumed["pending"] = True
            GLib.timeout_add(1500, resume)

    watch_streams(streams)
    portal.on_lost(lost)
    for sig in (signal.SIGTERM, signal.SIGINT):
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, sig, loop.quit)
    parent = os.getppid()

    def watch_parent():
        if os.getppid() != parent:
            loop.quit()
            return False
        return True

    GLib.timeout_add_seconds(2, watch_parent)
    GLib.timeout_add(max(500, int(args.interval * 1000)), tend)
    if args.once:
        # The user has a little while to choose; then give up.
        GLib.timeout_add_seconds(20, loop.quit)
    for watch in watched:
        connect(watch)
    emit(ev="ready", sources=sources, restore_token=getattr(portal, "restore_token", None))
    loop.run()
    # Not waiting for the streams to stop: one still waiting for the
    # compositor's first picture would hold the exit up. The portal session
    # is closed first; ending the process closes the streams.
    portal.close()
    sys.stdout.flush()
    os._exit(1 if args.once and counter["n"] == 0 else 0)


if __name__ == "__main__":
    sys.exit(main())
