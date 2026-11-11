/// Percentiles of a sample of integer values.
final class Percentiles {
  const Percentiles({
    required this.count,
    required this.p50,
    required this.p99,
    required this.p999,
    required this.max,
  });

  factory Percentiles.of(List<int> values) {
    if (values.isEmpty) {
      return const Percentiles(count: 0, p50: 0, p99: 0, p999: 0, max: 0);
    }
    final sorted = List<int>.of(values)..sort();
    int at(double q) => sorted[((sorted.length - 1) * q).round()];
    return Percentiles(
      count: sorted.length,
      p50: at(0.5),
      p99: at(0.99),
      p999: at(0.999),
      max: sorted.last,
    );
  }

  final int count;
  final int p50;
  final int p99;
  final int p999;
  final int max;
}
