// Seeds a `shop` database with realistic BSON for the nosqlsync end-to-end test.
// Run with: mongosh --quiet mongodb://localhost:27017/shop tests/e2e/seed.js
db.dropDatabase();

let s = 11;
const rand = () => {
  s = (s + 0x6d2b79f5) | 0;
  let t = Math.imul(s ^ (s >>> 15), 1 | s);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const pick = (a) => a[Math.floor(rand() * a.length)];

// 100k orders: ObjectId ids, every common BSON type, nested + array shapes.
const N = 20000;
const customers = Array.from({ length: 500 }, () => new ObjectId());
for (let start = 0; start < N; start += 5000) {
  const batch = [];
  for (let i = start; i < start + 5000; i++) {
    const d = {
      _id: new ObjectId(),
      customerId: pick(customers),
      createdAt: new Date(1.6e12 + i * 60000),
      status: pick(["paid", "pending", "refunded"]),
      qty: NumberInt(1 + (i % 7)),
      views: NumberLong(String(5000000000 + i)), // beyond 32 bits
      ratio: i % 2 ? 2.0 : 2.5, // a whole-number double must stay double
      price: NumberDecimal(((i % 10000) / 100).toFixed(2)),
      address: { city: pick(["Oslo", "Lima", "Pune"]), zip: String(1000 + (i % 9000)), geo: { lat: rand() * 90, lng: rand() * 180 } },
      items: Array.from({ length: 1 + (i % 3) }, (_, k) => ({ sku: "SKU-" + k, qty: NumberInt(k + 1), unit: 9.5, addedAt: new Date(1.6e12 + k) })),
      tags: i % 4 ? ["gift", "rush"].slice(0, i % 3) : [],
      thumb: BinData(0, "3q2+7w=="), // de ad be ef
      token: UUID(),
      seenAt: Timestamp({ t: 1700000000 + i, i: 1 }),
      note: i % 5 === 0 ? null : "n" + i,
      "first name": pick(["Ada", "Lin"]),
    };
    if (i % 3 === 0) d.couponCode = "C" + i;
    if (i % 10 === 0) d.meta = i % 20 ? { source: "web" } : { campaign: "x", ab: NumberInt(1) };
    batch.push(d);
  }
  db.orders.insertMany(batch, { ordered: false });
}

// String ids, and a numeric id collection: the non-ObjectId primary-key paths.
db.users.insertMany(
  Array.from({ length: 2000 }, (_, i) => ({
    _id: "user-" + String(i).padStart(6, "0") + "-with-a-long-identifier",
    email: "u" + i + "@x.io",
    profile: { first: "A", last: "B" },
    orgId: pick(customers),
    score: i % 2 ? NumberInt(i) : i + 0.5,
  }))
);
db.counters.insertMany(Array.from({ length: 50 }, (_, i) => ({ _id: NumberLong(String(i)), value: NumberInt(i * 2) })));

// A view, which must be skipped (it is not a collection).
db.createView("paid_orders", "orders", [{ $match: { status: "paid" } }]);

printjson({ orders: db.orders.countDocuments(), users: db.users.countDocuments(), counters: db.counters.countDocuments() });
