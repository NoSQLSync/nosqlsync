class Nosqlsync < Formula
  desc "Migrate MongoDB to PostgreSQL with a schema you'd actually design"
  homepage "https://github.com/NoSQLSync/nosqlsync"
  url "https://github.com/NoSQLSync/nosqlsync/archive/refs/tags/v0.1.1.tar.gz"
  sha256 "7649a9bcdbba247f465d9a8c25de4b1a08be3124c023f3ef5dc2c997cb366834"
  license any_of: ["MIT", "Apache-2.0"]
  head "https://github.com/NoSQLSync/nosqlsync.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "nosqlsync-cli")
  end

  test do
    assert_match "nosqlsync #{version}", shell_output("#{bin}/nosqlsync --version")

    (testpath/"orders.json").write <<~JSON
      [{"_id": {"$oid": "507f1f77bcf86cd799439011"}, "address": {"city": "Oslo"}, "items": [{"sku": "a", "qty": 1}]}]
    JSON
    output = shell_output("#{bin}/nosqlsync analyze --source #{testpath}/orders.json")
    assert_match "address_city", output
    assert_match "child table orders_items", output
  end
end
