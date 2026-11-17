/// The client's version, derived from the update notes so it moves with
/// every playable change: `0.<entries>` plus the date of the newest one
/// (`0.27 (2026-11-14)`). The server reports its own (`Connected`,
/// `/health`).
library;

import 'package:game_client/src/changelog.dart';

/// `0.<number of changelog entries>`.
final String clientVersion = '0.${changelog.length}';

/// [clientVersion] with the date of the newest update.
final String clientBuild = changelog.isEmpty ? clientVersion : '$clientVersion (${changelog.first.date})';

/// One line for the title screen and the settings: client and, once
/// known, server.
String versionLine(String? server) => [
  'Client $clientBuild',
  if (server != null && server.isNotEmpty) 'Server $server',
].join(' · ');
