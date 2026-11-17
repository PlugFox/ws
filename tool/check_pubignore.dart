// Checks that .pubignore repeats every .gitignore pattern.
//
// `dart pub publish` ignores .gitignore when a .pubignore exists, so a
// pattern missing from .pubignore would leak local files into the package.
// Gitignored paths that are published on purpose must be declared in
// .pubignore as `# publish: <pattern>`.
//
// Usage: dart run tool/check_pubignore.dart
import 'dart:io';

void main() {
  final git = _patterns(File('.gitignore').readAsLinesSync());
  final pubLines = File('.pubignore').readAsLinesSync();
  final pub = _patterns(pubLines);
  final declared = {
    for (final line in pubLines)
      if (line.trim().startsWith('# publish:'))
        line.trim().substring('# publish:'.length).trim(),
  };

  final missing = git.difference(pub).difference(declared);
  final stale = declared.difference(git);
  final contradictory = declared.intersection(pub);

  var ok = true;
  void fail(String title, Set<String> items) {
    if (items.isEmpty) return;
    ok = false;
    stderr.writeln(title);
    for (final p in items) {
      stderr.writeln('  $p');
    }
  }

  fail(
    '.gitignore patterns missing from .pubignore '
    '(add them, or declare "# publish: <pattern>"):',
    missing,
  );
  fail('"# publish:" exceptions that are not in .gitignore:', stale);
  fail('"# publish:" exceptions that .pubignore still ignores:', contradictory);
  if (!ok) exit(1);
  stdout.writeln(
    '.pubignore covers .gitignore '
    '(${git.length} patterns, ${declared.length} declared exceptions).',
  );
}

Set<String> _patterns(List<String> lines) => {
  for (final line in lines)
    if (line.trim() case final p when p.isNotEmpty && !p.startsWith('#')) p,
};
