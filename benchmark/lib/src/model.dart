/// Typical application models for the server's JSON payload, mapped with a
/// hand-written `fromJson` — what an app usually does with every message.
library;

final class Order {
  const Order({required this.seq, required this.ts, required this.items});

  factory Order.fromJson(Map<String, Object?> json) => Order(
        seq: json['seq']! as int,
        ts: json['ts']! as int,
        items: [
          for (final item in json['items']! as List<Object?>)
            Item.fromJson(item! as Map<String, Object?>),
        ],
      );

  final int seq;
  final int ts;
  final List<Item> items;

  Map<String, Object?> toJson() => {
        'seq': seq,
        'ts': ts,
        'items': [for (final item in items) item.toJson()],
      };
}

final class Item {
  const Item({
    required this.id,
    required this.name,
    required this.price,
    required this.qty,
    required this.active,
    required this.tags,
    required this.score,
    required this.rank,
    required this.note,
  });

  factory Item.fromJson(Map<String, Object?> json) {
    final meta = json['meta']! as Map<String, Object?>;
    return Item(
      id: json['id']! as int,
      name: json['name']! as String,
      price: (json['price']! as num).toDouble(),
      qty: json['qty']! as int,
      active: json['active']! as bool,
      tags: [for (final t in json['tags']! as List<Object?>) t! as String],
      score: (meta['score']! as num).toDouble(),
      rank: meta['rank']! as int,
      note: meta['note'] as String?,
    );
  }

  final int id;
  final String name;
  final double price;
  final int qty;
  final bool active;
  final List<String> tags;
  final double score;
  final int rank;
  final String? note;

  Map<String, Object?> toJson() => {
        'id': id,
        'name': name,
        'price': price,
        'qty': qty,
        'active': active,
        'tags': tags,
        'meta': {'score': score, 'rank': rank, 'note': note},
      };
}
