

# **Project AmateRS: Technical Architecture Whitepaper**

Version: 0.1.0 (Draft)

Author: COOLJAPAN OU (Team KitaSan)

Status: Confidential / Architecture Freeze

## **1\. Executive Summary**

AmateRS は、Rust言語によって記述される次世代の分散データベース基盤である。

従来のデータベースが「保存時の暗号化（Encryption at Rest）」に留まっていたのに対し、AmateRSは完全準同型暗号（FHE）を用いることで、\*\*「メモリ上での計算処理中も常に暗号化された状態（Encryption in Use）」\*\*を維持する。

これにより、クラウド事業者やシステム管理者であっても顧客データの生データ（Plaintext）にアクセスすることは数学的に不可能となり、真の「データ主権（Data Sovereignty）」を実現する。

---

## **2\. System Architecture Overview**

システムはマイクロカーネルアーキテクチャを採用し、以下の4つのコアモジュールによって構成される。

| モジュール名 | 日本神話の由来 | 機能役割 (Role) | 主要技術スタック |
| :---- | :---- | :---- | :---- |
| **Iwato** | 天岩戸 (Storage) | 暗号文に最適化された永続化層。LSM-Treeベース。 | io\_uring, rkyv, WiscKey |
| **Yata** | 八咫鏡 (Compute) | 暗号状態でのクエリ実行計画と演算処理。 | tfhe-rs, wgpu (CUDA/Metal), Rayon |
| **Ukehi** | 宇気比 (Consensus) | 暗号化ログの分散合意形成とクラスタ管理。 | Raft, Tonic (gRPC), zk-SNARKs |
| **Musubi** | 結び (Network) | クライアントとの通信、mTLS、プロトコル変換。 | QUIC, Rustls, Cap'n Proto |

---

## **3\. Detailed Component Specification**

### **3.1. Storage Engine: Iwato (The Rock Door)**

**Objective:** 数KB〜数MBに及ぶ巨大なFHE暗号文（Ciphertext）を高スループットで読み書きする。

* **Key-Value Separation (WiscKey Architecture):**  
  * 従来のLSM-Tree（LevelDB/RocksDB）はキーと値を一緒にソートするが、FHEの巨大な値を含めるとコンパクション（整理）時の書き込み増幅（Write Amplification）がHDD/SSDの寿命を削る。  
  * **設計:** LSM-Tree には「Key」と「Value Logへのポインタ（offset, size）」のみを格納。実データは追記型の vLog (Value Log) にシーケンシャル書き込みを行う。  
* **Direct I/O & Asynchronous Runtime:**  
  * OSのページキャッシュを経由せず、Rustの io\_uring ラッパー（glommio または tokio-uring）を用いてNVMe SSDへDMA転送を行う。  
* **Zero-Copy Deserialization:**  
  * rkyv を採用。ディスク上のバイナリフォーマットとメモリ上の構造体レイアウトを一致させ、mmap された領域へのポインタキャストのみでデータのロードを完了させる（Parseコスト 0）。

### **3.2. Execution Engine: Yata (The Mirror)**

**Objective:** 暗号化されたデータに対して、平文と同様の論理演算・算術演算を適用する。

* **Circuit Compilation (JIT):**  
  * クライアントから送られるクエリ（DSL）を、実行時にFHEの論理回路（Circuit）に変換する。  
  * **Optimizer:** 演算コストの高い「Bootstrapping（ノイズ除去）」の回数を最小化するように回路を再構成するコストベースオプティマイザを実装。  
* **Vectorized Execution (SIMD/GPU):**  
  * 単一のデータに対する演算ではなく、カラム（列）に対するバッチ処理を行う。  
  * tfhe-cuda-backend や wgpu を活用し、数千件のレコードに対する加算・比較をGPUで並列実行する。  
* **Scalar/Vector Hybrid:**  
  * メタデータ（IDやタグなど、暗号化不要な部分）と、センシティブデータ（金額、病歴など、FHE化部分）を分離して処理し、性能を最大化する。

### **3.3. Consensus Layer: Ukehi (The Pledge)**

**Objective:** ノード障害に耐えうる分散整合性の確保。

* **Encrypted Raft Protocol:**  
  * LeaderノードはLogの内容（コマンド）を解読できない。  
  * **検証:** クライアントが付与した「ハッシュ値」または「ゼロ知識証明（ZKP）」を用いて、データの中身を見ずにログの整合性（改ざんされていないこと）を検証する。  
* **Sharding & Partitioning:**  
  * データ量に応じてキー範囲（Key-Range）ごとにシャードを分割（Region）。  
  * Placement Driver (PD) コンポーネントが、各ノードの負荷状況（CPU使用率、FHE演算負荷）を監視し、動的にデータを再配置する。

### **3.4. Network & API: Musubi (The Knot)**

**Objective:** 言語非依存のインターフェース提供。

* **AmateRS Query Language (AQL):**  
  * SQLライクではなく、Rustのメソッドチェーンに近いDSLを定義。

// AQL Example  
db.collection("salaries")  
  .filter(col("department").eq("R\&D")) // 平文インデックス検索  
  .update(col("amount").add\_assign(5000.encrypt(pk))) // 暗号化加算

* **gRPC over QUIC:**  
  * HTTP/3ベースの通信により、パケットロス時のヘッドオブラインブロッキングを防ぎ、高レイテンシなネットワーク環境（海外拠点間など）でも安定稼働させる。

---

## **4\. Development Strategy (Implementation Blueprint)**

### **Phase 1: The Core (Kernel Implementation)**

* **期間:** 1ヶ月  
* **目標:** シングルノードでの Iwato \+ Yata の統合。  
* **成果物:**  
  * amaters-core クレート。  
  * ローカルディスクへの永続化と、CLI経由での SET, GET, ADD 動作確認。  
  * ベンチマークテスト（Ops/sec）。

### **Phase 2: Distributed (Cluster Implementation)**

* **期間:** 2〜3ヶ月  
* **目標:** Ukehi (Raft) の実装と複数ノード間でのレプリケーション。  
* **成果物:**  
  * gRPCサーバーの実装。  
  * Leader Election、Log Replicationの動作確認。  
  * カオスエンジニアリング（ノードを強制停止してもデータが壊れないか）。

### **Phase 3: Ecosystem (SDK & Tooling)**

* **期間:** 継続  
* **目標:** 開発者が使える状態にする。  
* **成果物:**  
  * Rust SDK, Python SDK (PyO3 bindings), TypeScript SDK (WASM)。  
  * Docker/Kubernetes Helm Charts。  
  * 管理画面（GUI）。

---

## **5\. Security Model & Threat Assessment**

| 脅威 (Threat) | 対策 (Countermeasure) |
| :---- | :---- |
| **サーバーへの物理侵入** | 全データはFHEで暗号化されており、メモリダンプを取得しても復号不能。 |
| **管理者による覗き見** | 秘密鍵はクライアント（ユーザー端末）にしか存在しないため不可能。 |
| **演算結果の改ざん** | サーバーは演算結果と共に「演算証明」を返す（将来的なVerifiable FHEへの対応）。 |
| **量子コンピュータ攻撃** | ZamaのTFHEは格子暗号（LWE）ベースであり、量子耐性（Post-Quantum）を持つ。 |

---

## **6\. Directory Structure (Monorepo)**

Plaintext

amaters/  
├── Cargo.toml (workspace)  
├── rust-toolchain.toml (nightly features enabled)  
├── docs/               \# Architecture Decision Records (ADR)  
├── core/               \# amaters-core: The Kernel  
│   ├── src/  
│   │   ├── storage/    \# Iwato Engine (LSM, Wal, BlockCache)  
│   │   ├── compute/    \# Yata Engine (TFHE circuits)  
│   │   └── types/      \# Common types (CipherBlob)  
├── net/                \# amaters-net: Network Layer  
│   ├── src/            \# gRPC defs, Connection Pooling  
├── cluster/            \# amaters-cluster: Consensus  
│   ├── src/            \# Raft implementation, Sharding logic  
├── server/             \# amaters-server: The Binary  
│   ├── src/            \# main.rs, Config loading  
├── sdk/                \# Client SDKs  
│   ├── rust/  
   └── python/

---

### **SLOC見積もり詳細（内訳）**

| レイヤー | モジュール名 | 推定SLOC (Rust) | 解説 |
| :---- | :---- | :---- | :---- |
| **Storage** | **Iwato** | **15,000 \- 25,000** | 最も重厚な部分です。LSM-Treeの実装、WAL、クラッシュリカバリ、io\_uringの非同期制御、ガベージコレクション。ここだけで単体のOSSプロジェクト（例: Sled）規模です。 |
| **Compute** | **Yata** | **8,000 \- 12,000** | FHE演算回路の最適化ロジック、クエリプランナー、GPUアクセラレーションのグルーコード。tfhe-rs があるため基礎研究部分は省けますが、応用ロジックは厚くなります。 |
| **Consensus** | **Ukehi** | **10,000 \- 15,000** | Raftアルゴリズム（または raft-rs のラッパー）、リーダー選出、スナップショット管理、ネットワーク分断時のリカバリ処理。分散システムの肝です。 |
| **Network** | **Musubi** | **5,000 \- 8,000** | gRPC定義、エラーハンドリング、コネクションプール、認証認可（mTLS）。 |
| **SDK/CLI** | **Tools** | **5,000 \- 10,000** | 開発者用ツール、クライアントライブラリ、管理画面バックエンド。 |
| **Testing** | **Test Suite** | **30,000 \- 50,000** | **ここが重要です。** DB製品は本体コードと同量以上のテスト（ユニット、統合、ファジング、カオスエンジニアリング）が必要です。 |
| **合計** |  | **約 7万 〜 12万行** | **V1.0 リリース時点での想定規模** |

---

Rust 例：

// core/src/error.rs  
use thiserror::Error;

\#\[derive(Error, Debug)\]  
pub enum AmateRSError {  
    \#\[error("Storage I/O integrity violation: {0}")\]  
    StorageIntegrity(String),  
      
    \#\[error("FHE Computation failed: {0}")\]  
    FheComputation(String),  
      
    \#\[error("Consensus log divergence detected at index {0}")\]  
    ConsensusDivergence(u64),

    \#\[error("Serialization error: {0}")\]  
    Serialization(\#\[from\] rkyv::rancor::Error),  
      
    // 他のいかなる「想定外」も許さない  
    \#\[error("Critical system invariant broken: {0}")\]  
    SystemInvariantBroken(String),  
}

pub type Result\<T\> \= std::result::Result\<T, AmateRSError\>;

Rust例：

// core/src/traits.rs

use crate::types::{CipherBlob, Key};  
use crate::error::Result;

/// 永続化層の「契約」。  
/// 実装がいかなる方法（LSM, B-Tree）であれ、この契約は絶対である。  
\#\[async\_trait::async\_trait\]  
pub trait StorageEngine: Send \+ Sync \+ 'static {  
    /// データの書き込み。  
    /// 成功した場合は「永続化されたことがディスク上で保証された」ことを意味する（fsync相当）。  
    async fn put(\&self, key: \&Key, value: \&CipherBlob) \-\> Result\<()\>;

    /// データの読み出し。  
    /// データが存在しないことはエラーではなく Option::None である。  
    /// 破損している場合は Error::StorageIntegrity を返すこと。  
    async fn get(\&self, key: \&Key) \-\> Result\<Option\<CipherBlob\>\>;  
      
    /// アトミックな演算更新。  
    /// Read-Modify-Write の競合を厳密に排除する。  
    async fn atomic\_update\<F\>(\&self, key: \&Key, f: F) \-\> Result\<()\>  
    where   
        F: Fn(\&CipherBlob) \-\> Result\<CipherBlob\> \+ Send \+ Sync;  
}

