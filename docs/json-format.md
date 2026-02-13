# 衝立詰将棋ソルバー JSONフォーマット仕様

アプリのインポート/エクスポート機能で使用するJSONフォーマットの説明です。

## フォーマット

```json
{
  "board": [
    { "file": 2, "rank": 2, "color": "gote", "kind": "king" },
    { "file": 2, "rank": 1, "color": "gote", "kind": "gold" },
    { "file": 1, "rank": 2, "color": "gote", "kind": "rook" },
    { "file": 1, "rank": 3, "color": "gote", "kind": "pawn" },
    { "file": 4, "rank": 1, "color": "gote", "kind": "silver" },
    { "file": 4, "rank": 3, "color": "gote", "kind": "pawn" },
    { "file": 2, "rank": 5, "color": "sente", "kind": "pawn" },
    { "file": 3, "rank": 4, "color": "sente", "kind": "pawn" }
  ],
  "sente_hand": [
    { "kind": "silver", "count": 1 },
    { "kind": "knight", "count": 1 }
  ]
}
```

## フィールド説明

### `board` (必須)

盤上の駒の配列です。空きマスは含めず、駒のあるマスのみ列挙します。

| フィールド | 型 | 説明 |
|-----------|------|------|
| `file` | number (1-9) | 筋。右から左へ 1〜9 |
| `rank` | number (1-9) | 段。上から下へ 1〜9 |
| `color` | string | `"sente"` (先手/攻め方) または `"gote"` (後手/玉方) |
| `kind` | string | 駒の種類（下表参照） |

### `sente_hand` (任意)

先手（攻め方）の持ち駒の配列です。省略した場合、持ち駒なしとして扱います。

| フィールド | 型 | 説明 |
|-----------|------|------|
| `kind` | string | 駒の種類（持ち駒に使える駒のみ。下表参照） |
| `count` | number | 枚数 |

> **後手の持ち駒について**: 後手（玉方）の持ち駒はJSONに含める必要はありません。詰将棋のルールに従い、盤上と先手持ち駒に含まれない駒は全て後手の持ち駒として自動計算されます。

## 駒の種類 (`kind`)

### 盤上の駒（`board` で使用可能）

| `kind` | 日本語名 |
|--------|---------|
| `king` | 玉 |
| `rook` | 飛 |
| `bishop` | 角 |
| `gold` | 金 |
| `silver` | 銀 |
| `knight` | 桂 |
| `lance` | 香 |
| `pawn` | 歩 |
| `promoted_rook` | 竜 |
| `promoted_bishop` | 馬 |
| `promoted_silver` | 成銀 |
| `promoted_knight` | 成桂 |
| `promoted_lance` | 成香 |
| `promoted_pawn` | と |

### 持ち駒（`sente_hand` で使用可能）

成駒は持ち駒にできません。以下の7種のみ有効です。

| `kind` | 日本語名 | 最大枚数 |
|--------|---------|---------|
| `rook` | 飛 | 2 |
| `bishop` | 角 | 2 |
| `gold` | 金 | 4 |
| `silver` | 銀 | 4 |
| `knight` | 桂 | 4 |
| `lance` | 香 | 4 |
| `pawn` | 歩 | 18 |

## 座標系

```
        9    8    7    6    5    4    3    2    1   ← file（筋）
      +----+----+----+----+----+----+----+----+----+
  1   |    |    |    |    |    |    |    |    |    |  ← rank（段）
      +----+----+----+----+----+----+----+----+----+
  2   |    |    |    |    |    |    |    |    |    |
      +----+----+----+----+----+----+----+----+----+
  3   |    |    |    |    |    |    |    |    |    |
      +----+----+----+----+----+----+----+----+----+
  ...
      +----+----+----+----+----+----+----+----+----+
  9   |    |    |    |    |    |    |    |    |    |
      +----+----+----+----+----+----+----+----+----+
```

- `file` (筋): 1〜9。将棋の標準表記と同じで、右から左へ数える
- `rank` (段): 1〜9。上から下へ数える（一段目が上）
- 例: `{ "file": 2, "rank": 2 }` → ２二のマス

## バリデーション

インポート時に以下が確認されます:

- `board` 配列が存在すること
- `file` が 1〜9 の範囲内であること
- `rank` が 1〜9 の範囲内であること
- `color` が `"sente"` または `"gote"` であること
- `kind` が有効な駒の種類であること

求解時にはさらに以下が確認されます:

- 後手の玉（`"gote"` の `"king"`）が盤上にあること

## 例

### 1手詰め（頭金）

```json
{
  "board": [
    { "file": 1, "rank": 1, "color": "gote", "kind": "king" },
    { "file": 2, "rank": 2, "color": "sente", "kind": "gold" }
  ],
  "sente_hand": [
    { "kind": "gold", "count": 1 }
  ]
}
```

▲２一金打で詰み。

### 3手詰め（反則利用の衝立詰将棋）

```json
{
  "board": [
    { "file": 6, "rank": 7, "color": "sente", "kind": "pawn" },
    { "file": 6, "rank": 8, "color": "sente", "kind": "silver" },
    { "file": 7, "rank": 5, "color": "sente", "kind": "pawn" },
    { "file": 8, "rank": 3, "color": "sente", "kind": "bishop" },
    { "file": 8, "rank": 5, "color": "gote", "kind": "king" },
    { "file": 8, "rank": 7, "color": "sente", "kind": "pawn" },
    { "file": 9, "rank": 3, "color": "gote", "kind": "pawn" },
    { "file": 9, "rank": 6, "color": "sente", "kind": "pawn" }
  ],
  "sente_hand": [
    { "kind": "bishop", "count": 1 }
  ]
}
```

▲７四角成 → (NoCapture) → ▲７六角打 → (Illegal/Checkmate分岐) で詰み。
