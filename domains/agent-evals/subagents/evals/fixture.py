"""The small project every task starts from (written fresh per attempt)."""

FILES = {
    "inventory/__init__.py": "",
    "inventory/__main__.py": "from .cli import main\n\nmain()\n",
    "inventory/store.py": '''import json
from pathlib import Path


def load_items(path):
    return json.loads(Path(path).read_text())


def total_quantity(items):
    total = 0
    for i in range(len(items) - 1):
        total += items[i]["qty"]
    return total


def find(items, sku):
    for item in items:
        if item["sku"] == sku:
            return item
    return None
''',
    "inventory/pricing.py": '''def line_price(item):
    return item["price"] * item["qty"]


def order_total(items):
    return sum(line_price(i) for i in items)


def average_price(items):
    return sum(i["price"] for i in items) / len(items)
''',
    "inventory/report.py": '''from .store import load_items, total_quantity
from .pricing import order_total


def summary(path):
    items = load_items(path)
    return {"count": len(items), "qty": total_quantity(items), "total": order_total(items)}


def low_stock(path, threshold):
    items = load_items(path)
    return [i["sku"] for i in items if i["qty"] < threshold]
''',
    "inventory/cli.py": '''import argparse
import json

from .report import summary


def main(argv=None):
    p = argparse.ArgumentParser(prog="inventory")
    p.add_argument("path")
    p.add_argument("--json", action="store_true", help="print the summary as JSON")
    args = p.parse_args(argv)
    s = summary(args.path)
    if args.json:
        print(json.dumps(s))
    else:
        print(f"{s['count']} items, {s['qty']} units, total {s['total']:.2f}")
''',
    "inventory/shipping.py": '''def shipping_cost(weight_kg, zone):
    """Shipping cost in dollars.

    zone "local": 5.00 flat up to 2 kg, then 1.50 per started kg over 2.
    zone "national": 9.00 flat up to 2 kg, then 2.50 per started kg over 2.
    Any other zone raises ValueError. Weight <= 0 raises ValueError.
    """
    raise NotImplementedError
''',
    "tests/__init__.py": "",
    "tests/test_store.py": '''import unittest

from inventory.store import find, total_quantity

ITEMS = [{"sku": "a", "qty": 2, "price": 1.0}, {"sku": "b", "qty": 3, "price": 2.0}]


class StoreTest(unittest.TestCase):
    def test_total_quantity(self):
        self.assertEqual(total_quantity(ITEMS), 5)

    def test_find(self):
        self.assertEqual(find(ITEMS, "b")["qty"], 3)
        self.assertIsNone(find(ITEMS, "z"))
''',
    "tests/test_pricing.py": '''import unittest

from inventory.pricing import order_total

ITEMS = [{"sku": "a", "qty": 2, "price": 1.0}, {"sku": "b", "qty": 3, "price": 2.0}]


class PricingTest(unittest.TestCase):
    def test_order_total(self):
        self.assertEqual(order_total(ITEMS), 8.0)
''',
    "items.json": '[{"sku": "a", "qty": 2, "price": 1.0}, {"sku": "b", "qty": 3, "price": 2.0}, {"sku": "c", "qty": 1, "price": 10.0}]\n',
    "README.md": """# inventory

A small inventory tool.

## Usage

```
python3 -m inventory items.json
```

Prints the item count, the units in stock and the order total.
""",
}
