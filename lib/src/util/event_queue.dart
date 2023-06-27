import 'dart:async';
import 'dart:collection';

import 'package:meta/meta.dart';
import 'package:ws/src/util/logger.dart';

/// {@nodoc}
@internal
final class WebSocketEventQueue {
  /// {@nodoc}
  WebSocketEventQueue();

  final DoubleLinkedQueue<WebSocketTask<Object?>> _queue =
      DoubleLinkedQueue<WebSocketTask<Object?>>();
  Future<void>? _processing;

  /// Push it at the end of the queue.
  /// {@nodoc}
  Future<T> push<T>(String id, FutureOr<T> Function() fn) {
    final task = WebSocketTask<T>(id, fn);
    _queue.add(task);
    _exec();
    return task.future;
  }

  /// Push it at the end of the queue if it doesn't exist.
  /// Otherwise, return the result from existing one.
  /// {@nodoc}
  Future<T> pushIfNotExists<T>(String id, FutureOr<T> Function() fn) {
    if (_queue.isEmpty) return push<T>(id, fn);
    final iter = _queue.iterator;
    WebSocketTask<Object?> current;
    while (iter.moveNext()) {
      current = iter.current;
      if (current.id == id && current is WebSocketTask<T>) {
        return current.future;
      }
    }
    return push<T>(id, fn);
  }

  /// Clear the queue.
  /// {@nodoc}
  void clear() => _queue.clear();

  /// Execute the queue.
  /// {@nodoc}
  void _exec() => _processing ??= Future.doWhile(() async {
        final event = _queue.first;
        try {
          await event();
        } on Object catch (error, stackTrace) {
          warning(
            error,
            stackTrace,
            'Error while processing event "${event.id}"',
          );
        }
        _queue.removeFirst();
        final isEmpty = _queue.isEmpty;
        if (isEmpty) _processing = null;
        return !isEmpty;
      });
}

/// {@nodoc}
@internal
class WebSocketTask<T> {
  /// {@nodoc}
  WebSocketTask(this.id, FutureOr<T> Function() fn)
      : _fn = fn,
        _completer = Completer<T>();

  /// {@nodoc}
  final Completer<T> _completer;

  /// {@nodoc}
  final String id;

  /// {@nodoc}
  final FutureOr<T> Function() _fn;

  /// {@nodoc}
  Future<T> get future => _completer.future;

  /// {@nodoc}
  FutureOr<T> call() => (_completer..complete(_fn())).future;
}
