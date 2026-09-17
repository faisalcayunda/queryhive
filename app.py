#!/usr/bin/env python3
"""Desktop / localhost launcher: start the server on a free port, open a browser.

This is the entry point PyInstaller wraps into the .app inside the DMG. Running
it directly (`python app.py`) is the same thing without the bundle.
"""

from __future__ import annotations

import os
import socket
import sys
import threading
import webbrowser

DEFAULT_PORT = 8765


def free_port(preferred: int = DEFAULT_PORT) -> int:
    for port in (preferred, 0):
        with socket.socket() as s:
            try:
                s.bind(("127.0.0.1", port))
                return s.getsockname()[1]
            except OSError:
                continue
    raise RuntimeError("no port available")


def main() -> None:
    # a windowed .app has no console; uvicorn's logger needs a real stream
    for name in ("stdout", "stderr"):
        if getattr(sys, name, None) is None:
            setattr(sys, name, open(os.devnull, "w"))

    import uvicorn
    import webview
    from webview.menu import Menu, MenuAction
    from exporter.web import app

    port = int(os.getenv("QUERYHIVE_PORT") or os.getenv("TRINO_EXPORTER_PORT") or free_port())
    url = f"http://127.0.0.1:{port}"
    print(f"QueryHive → {url}", flush=True)

    def run_server():
        uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning")

    # Start the FastAPI server in a background thread
    server_thread = threading.Thread(target=run_server, daemon=True)
    server_thread.start()

    if "--no-browser" not in sys.argv:
        # Create native menu for Settings
        def open_settings():
            window.evaluate_js('openSettingsModal()')
            
        menu_items = [
            Menu('Settings', [
                MenuAction('Preferences...', open_settings)
            ])
        ]
        
        # Create a native window using pywebview instead of a browser
        window = webview.create_window('QueryHive', url, width=1100, height=800, min_size=(800, 600))
        webview.start(menu=menu_items)
    else:
        server_thread.join()


if __name__ == "__main__":
    main()
