/// Turns off the browser's own page gestures over the game: the context
/// menu, zoom (Ctrl/⌘ + wheel or + / − / 0, pinch, Safari's `gesture*`),
/// dragging images and interface pieces out, and selecting text with a
/// long press. Text fields (chat, sign-in, feedback) keep their native
/// behaviour: selection, the context menu, copy and paste.
///
/// The CSS half (`touch-action`, `overscroll-behavior`, `user-select`,
/// `-webkit-touch-callout`, the viewport meta) is in `web/index.html`; this
/// covers what CSS cannot. Listeners only `preventDefault()`: the game's
/// own handlers (right-click player menu on the canvas, the camera's
/// wheel zoom, the touch controls' synthetic `contextmenu`) still run.
library;

import 'dart:js_interop';

import 'package:web/web.dart' as web;

/// Where the browser's text behaviour stays on.
const _editable = 'input, textarea, select, [contenteditable]:not([contenteditable="false"])';

bool _inEditable(web.Event e) {
  final t = e.target;
  return t != null && t.isA<web.Element>() && (t as web.Element).closest(_editable) != null;
}

/// Keys that zoom the page with Ctrl / ⌘ (`Equal` is "+" without Shift).
const _zoomKeys = {'Equal', 'Minus', 'Digit0', 'NumpadAdd', 'NumpadSubtract', 'Numpad0'};

/// Whether a `keydown` is a browser zoom shortcut.
bool isZoomKey({required String code, required bool ctrl, required bool meta}) => (ctrl || meta) && _zoomKeys.contains(code);

bool _installed = false;

/// Installs the guards on the document (once).
void guardPage() {
  if (_installed) return;
  _installed = true;
  final opts = web.AddEventListenerOptions(capture: true, passive: false);
  void on(web.EventTarget target, String type, void Function(web.Event) fn) => target.addEventListener(type, fn.toJS, opts);

  on(web.document, 'contextmenu', (e) {
    if (!_inEditable(e)) e.preventDefault();
  });
  on(web.document, 'dragstart', (e) {
    if (!_inEditable(e)) e.preventDefault();
  });
  // Long-press / double-tap selection outside text fields (iOS ignores
  // `user-select` on some elements).
  on(web.document, 'selectstart', (e) {
    if (!_inEditable(e)) e.preventDefault();
  });
  on(web.window, 'wheel', (e) {
    final w = e as web.WheelEvent;
    if (w.ctrlKey || w.metaKey) e.preventDefault();
  });
  on(web.window, 'keydown', (e) {
    final k = e as web.KeyboardEvent;
    if (isZoomKey(code: k.code, ctrl: k.ctrlKey, meta: k.metaKey)) e.preventDefault();
  });
  // Safari's pinch (it ignores `user-scalable=no`).
  for (final type in const ['gesturestart', 'gesturechange', 'gestureend']) {
    on(web.document, type, (e) => e.preventDefault());
  }
  // A second finger on the page itself (not a game control) would pinch.
  on(web.document, 'touchmove', (e) {
    final t = e as web.TouchEvent;
    if (t.touches.length > 1 && !_inEditable(e)) e.preventDefault();
  });
}
