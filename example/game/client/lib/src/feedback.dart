/// The feedback form (the Scribe, or a Kafra's "Leave feedback"): what the
/// engine window shows (`showFeedback`), checked against the server's
/// limits before it goes out as [ClientMsg.feedback]. The server checks
/// again, rate limits per account and answers [FeedbackResult].
library;

import 'dart:convert';
import 'dart:typed_data';

import 'package:game_client/src/protocol.dart';
import 'package:game_client/src/version.dart';

/// Report kinds (server `feedback::KINDS`): the form's choices, and the
/// Scribe's menu ([prompt], [detail]) that opens the form on one of them.
typedef FeedbackKind = ({String id, String label, String prompt, String detail});

const feedbackKinds = <FeedbackKind>[
  (id: 'bug', label: 'Bug', prompt: 'Report a bug', detail: 'Something broke, froze or looks wrong.'),
  (id: 'suggestion', label: 'Suggestion', prompt: 'Suggest an idea', detail: 'Something you would like to see or change.'),
  (id: 'balance', label: 'Balance', prompt: 'Balance feedback', detail: 'Too hard, too easy, too strong, too weak.'),
  (id: 'other', label: 'Other', prompt: 'Something else', detail: 'Anything the developers should know.'),
];

/// Priorities (server `feedback::PRIORITIES`), as the player sees them.
const feedbackPriorities = <({String id, String label})>[
  (id: 'low', label: 'Low'),
  (id: 'normal', label: 'Normal'),
  (id: 'high', label: 'High'),
  (id: 'critical', label: 'Critical'),
];

/// One open form: its choices, whether it waits for the server, the last
/// error. The engine keeps what was typed across re-renders.
final class FeedbackForm {
  FeedbackForm({required this.npc, String kind = ''}) : kind = feedbackKinds.any((k) => k.id == kind) ? kind : 'bug';

  /// The NPC it was opened at.
  final String npc;
  String kind;
  String priority = 'normal';
  bool busy = false;
  String? error;

  /// `FeedbackSpec` JSON for the engine.
  String spec() => jsonEncode({
    'kind': kind,
    'priority': priority,
    'kinds': [
      for (final k in feedbackKinds) {'id': k.id, 'label': k.label},
    ],
    'priorities': [
      for (final p in feedbackPriorities) {'id': p.id, 'label': p.label},
    ],
    'titleMin': FeedbackLimits.titleMin,
    'titleMax': FeedbackLimits.titleMax,
    'bodyMin': FeedbackLimits.bodyMin,
    'bodyMax': FeedbackLimits.bodyMax,
    'version': clientBuild,
    'busy': busy,
    'error': ?error,
  });

  /// A `feedback_submit {kind, priority, title, body}` event: the frame to
  /// send (now busy), or null with [error] set.
  Uint8List? submit(Map<String, Object?> e, {required String userAgent}) {
    kind = '${e['kind'] ?? kind}';
    priority = '${e['priority'] ?? priority}';
    final title = '${e['title'] ?? ''}'.trim().replaceAll(RegExp(r'\s+'), ' ');
    final body = '${e['body'] ?? ''}'.trim();
    error = switch ((title.runes.length, body.runes.length)) {
      (final t, _) when t < FeedbackLimits.titleMin || t > FeedbackLimits.titleMax =>
        'The title needs ${FeedbackLimits.titleMin} to ${FeedbackLimits.titleMax} characters.',
      (_, final b) when b < FeedbackLimits.bodyMin || b > FeedbackLimits.bodyMax =>
        'Describe it in ${FeedbackLimits.bodyMin} to ${FeedbackLimits.bodyMax} characters.',
      _ => null,
    };
    if (error != null) return null;
    busy = true;
    return ClientMsg.feedback(
      kind: kind,
      priority: priority,
      title: title,
      body: body,
      clientVersion: clientBuild,
      userAgent: userAgent.length > FeedbackLimits.userAgentMax ? userAgent.substring(0, FeedbackLimits.userAgentMax) : userAgent,
    );
  }

  /// The server refused it: show why, keep the text.
  void refused(String message) {
    busy = false;
    error = message;
  }
}
