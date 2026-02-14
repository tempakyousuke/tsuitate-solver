use serde::{Deserialize, Serialize};

/// 先手 (Sente / Attacker) or 後手 (Gote / Defender/King side)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Color {
    Sente, // 先手 (攻め方)
    Gote,  // 後手 (玉方)
}

impl Color {
    pub fn opponent(self) -> Color {
        match self {
            Color::Sente => Color::Gote,
            Color::Gote => Color::Sente,
        }
    }
}

/// 駒の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PieceKind {
    King,           // 玉
    Rook,           // 飛
    Bishop,         // 角
    Gold,           // 金
    Silver,         // 銀
    Knight,         // 桂
    Lance,          // 香
    Pawn,           // 歩
    PromotedRook,   // 竜
    PromotedBishop, // 馬
    PromotedSilver, // 成銀
    PromotedKnight, // 成桂
    PromotedLance,  // 成香
    PromotedPawn,   // と
}

impl PieceKind {
    /// 成ることができるか
    pub fn can_promote(self) -> bool {
        matches!(
            self,
            PieceKind::Rook
                | PieceKind::Bishop
                | PieceKind::Silver
                | PieceKind::Knight
                | PieceKind::Lance
                | PieceKind::Pawn
        )
    }

    /// 成った後の駒種
    pub fn promoted(self) -> Option<PieceKind> {
        match self {
            PieceKind::Rook => Some(PieceKind::PromotedRook),
            PieceKind::Bishop => Some(PieceKind::PromotedBishop),
            PieceKind::Silver => Some(PieceKind::PromotedSilver),
            PieceKind::Knight => Some(PieceKind::PromotedKnight),
            PieceKind::Lance => Some(PieceKind::PromotedLance),
            PieceKind::Pawn => Some(PieceKind::PromotedPawn),
            _ => None,
        }
    }

    /// 成り駒の元の駒種
    pub fn unpromoted(self) -> PieceKind {
        match self {
            PieceKind::PromotedRook => PieceKind::Rook,
            PieceKind::PromotedBishop => PieceKind::Bishop,
            PieceKind::PromotedSilver => PieceKind::Silver,
            PieceKind::PromotedKnight => PieceKind::Knight,
            PieceKind::PromotedLance => PieceKind::Lance,
            PieceKind::PromotedPawn => PieceKind::Pawn,
            other => other,
        }
    }

    /// 持ち駒にできる駒種か（玉以外の非成り駒）
    pub fn is_hand_piece(self) -> bool {
        matches!(
            self,
            PieceKind::Rook
                | PieceKind::Bishop
                | PieceKind::Gold
                | PieceKind::Silver
                | PieceKind::Knight
                | PieceKind::Lance
                | PieceKind::Pawn
        )
    }

    /// 日本語表記
    pub fn to_kanji(self) -> &'static str {
        match self {
            PieceKind::King => "玉",
            PieceKind::Rook => "飛",
            PieceKind::Bishop => "角",
            PieceKind::Gold => "金",
            PieceKind::Silver => "銀",
            PieceKind::Knight => "桂",
            PieceKind::Lance => "香",
            PieceKind::Pawn => "歩",
            PieceKind::PromotedRook => "竜",
            PieceKind::PromotedBishop => "馬",
            PieceKind::PromotedSilver => "成銀",
            PieceKind::PromotedKnight => "成桂",
            PieceKind::PromotedLance => "成香",
            PieceKind::PromotedPawn => "と",
        }
    }

    /// All hand piece kinds in standard order
    pub const HAND_PIECES: [PieceKind; 7] = [
        PieceKind::Rook,
        PieceKind::Bishop,
        PieceKind::Gold,
        PieceKind::Silver,
        PieceKind::Knight,
        PieceKind::Lance,
        PieceKind::Pawn,
    ];
}

/// 駒
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Piece {
    pub color: Color,
    pub kind: PieceKind,
}

impl Piece {
    pub fn new(color: Color, kind: PieceKind) -> Self {
        Self { color, kind }
    }
}

/// マス目 (筋: file 1-9, 段: rank 1-9)
/// 将棋の座標系: file は右から左 (9→1), rank は上から下 (1→9)
/// 先手から見て、1一が右上、9九が左下
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Square {
    pub file: u8, // 筋 (1-9)
    pub rank: u8, // 段 (1-9)
}

impl Square {
    pub fn new(file: u8, rank: u8) -> Self {
        debug_assert!((1..=9).contains(&file) && (1..=9).contains(&rank));
        Self { file, rank }
    }

    /// 盤内かどうか
    pub fn is_valid(file: i8, rank: i8) -> bool {
        (1..=9).contains(&file) && (1..=9).contains(&rank)
    }

    /// 配列インデックスに変換 (0-80)
    pub fn index(self) -> usize {
        ((self.file - 1) * 9 + (self.rank - 1)) as usize
    }

    /// インデックスからSquareに変換
    pub fn from_index(idx: usize) -> Self {
        Self {
            file: (idx / 9) as u8 + 1,
            rank: (idx % 9) as u8 + 1,
        }
    }

    /// 日本語表記 (例: "２三")
    pub fn to_japanese(self) -> String {
        let file_chars = ['１', '２', '３', '４', '５', '６', '７', '８', '９'];
        let rank_chars = ['一', '二', '三', '四', '五', '六', '七', '八', '九'];
        format!(
            "{}{}",
            file_chars[(self.file - 1) as usize],
            rank_chars[(self.rank - 1) as usize]
        )
    }
}

/// 指し手
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Move {
    /// 移動先
    pub to: Square,
    /// 移動元 (Noneの場合は打ち)
    pub from: Option<Square>,
    /// 成るかどうか
    pub promotion: bool,
    /// 打ちの場合の駒種
    pub drop_piece: Option<PieceKind>,
    /// 移動する駒の種類（成る前の駒種、表示用）
    pub moved_piece_kind: Option<PieceKind>,
}

impl Move {
    /// 盤上の駒の移動
    pub fn normal(from: Square, to: Square, promotion: bool, kind: PieceKind) -> Self {
        Self {
            from: Some(from),
            to,
            promotion,
            drop_piece: None,
            moved_piece_kind: Some(kind),
        }
    }

    /// 駒を打つ
    pub fn drop(to: Square, kind: PieceKind) -> Self {
        Self {
            from: None,
            to,
            promotion: false,
            drop_piece: Some(kind),
            moved_piece_kind: None,
        }
    }

    /// 日本語表記
    pub fn to_japanese(self, color: Color) -> String {
        let prefix = match color {
            Color::Sente => "▲",
            Color::Gote => "△",
        };
        let to_str = self.to.to_japanese();
        if let Some(kind) = self.drop_piece {
            format!("{}{}{}打", prefix, to_str, kind.to_kanji())
        } else {
            let kind_str = self
                .moved_piece_kind
                .map(|k| k.to_kanji())
                .unwrap_or("");
            let promo = if self.promotion { "成" } else { "" };
            format!("{}{}{}{}", prefix, to_str, kind_str, promo)
        }
    }
}

/// 持ち駒
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HandPieces {
    counts: [u8; 7], // Rook, Bishop, Gold, Silver, Knight, Lance, Pawn の順
}

impl HandPieces {
    pub fn new() -> Self {
        Self { counts: [0; 7] }
    }

    fn kind_index(kind: PieceKind) -> usize {
        match kind {
            PieceKind::Rook => 0,
            PieceKind::Bishop => 1,
            PieceKind::Gold => 2,
            PieceKind::Silver => 3,
            PieceKind::Knight => 4,
            PieceKind::Lance => 5,
            PieceKind::Pawn => 6,
            _ => panic!("Invalid hand piece kind: {:?}", kind),
        }
    }

    pub fn count(&self, kind: PieceKind) -> u8 {
        self.counts[Self::kind_index(kind)]
    }

    pub fn add(&mut self, kind: PieceKind) {
        let k = kind.unpromoted();
        self.counts[Self::kind_index(k)] += 1;
    }

    pub fn remove(&mut self, kind: PieceKind) {
        let idx = Self::kind_index(kind);
        debug_assert!(self.counts[idx] > 0);
        self.counts[idx] -= 1;
    }

    pub fn has(&self, kind: PieceKind) -> bool {
        self.counts[Self::kind_index(kind)] > 0
    }
}

impl Default for HandPieces {
    fn default() -> Self {
        Self::new()
    }
}
