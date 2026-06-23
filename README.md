# Bank

![Banking platform hero](assets/readme/banking-platform-hero.png)

A professional Rust banking practice project evolved from a simple CLI exercise into a layered, testable banking application with identity, role-based authorization, audit logging, REST APIs, SQLite persistence, and token-based API sessions.

This repository is an educational backend project, not production financial software.

## Language

- [English](#english)
- [فارسی](#فارسی)

## English

### Overview

`bank` is a Rust application that models core banking operations while keeping the codebase clean, modular, and testable. It includes a CLI for local workflows, an Axum HTTP API for product-facing access, durable JSON and SQLite storage, operational audit trails, and API hardening features.

### Highlights

- Cent-based money model instead of floating point arithmetic.
- Account lifecycle: create, update profile, activate, deactivate, close, and delete.
- Banking operations: deposit, withdraw, transfer, fees, interest, loan request, and loan payment.
- Customer identity records and role-based permissions for customer, teller, and admin users.
- Argon2 password hashing.
- Token-based API sessions with Bearer authentication and logout/revocation.
- Audit log with actor, action, outcome, target, message, and timestamp.
- JSON snapshots plus normalized SQLite persistence with migrations and indexes.
- Queryable audit and transaction history endpoints.
- Request IDs, structured tracing, and login throttling for API hardening.
- Unit and API tests covering core banking, identity, storage, SQLite migration, and authorization flows.

### Architecture

```text
src/
  api.rs           Axum REST API, Bearer sessions, request tracing, API tests
  app.rs           Application state composition
  audit.rs         Audit log domain model
  bank.rs          Core banking service and business rules
  cli.rs           Interactive command-line client
  domain.rs        Money, account, transaction, and shared domain types
  identity.rs      Customers, users, roles, permissions, authentication
  sqlite_store.rs  SQLite schema migrations and normalized persistence
  storage.rs       JSON snapshots and statement export
```

### API Authentication

The API starts empty. Bootstrap the first admin, then use the returned access token:

```bash
curl -X POST http://127.0.0.1:3000/auth/bootstrap-admin \
  -H "content-type: application/json" \
  -d '{"user_id":1,"username":"admin","password":"correct-password"}'
```

Protected endpoints require:

```text
Authorization: Bearer <access_token>
```

Logout revokes the current token:

```bash
curl -X POST http://127.0.0.1:3000/auth/logout \
  -H "authorization: Bearer <access_token>"
```

### Running

Start the CLI:

```bash
cargo run
```

Start the HTTP API:

```bash
cargo run -- serve 127.0.0.1:3000
```

Run development checks:

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

### Completed Phases

| Phase | Status | Summary |
| --- | --- | --- |
| 1 | Complete | Core banking foundation, typed errors, cent-based money, account operations |
| 2 | Complete | JSON storage, statement export, persistence tests |
| 3 | Complete | Customers, users, roles, permissions, Argon2 password hashing |
| 4 | Complete | Axum REST API, protected banking and identity endpoints |
| 5 | Complete | CI checks, transaction IDs, timestamps, audit log |
| 6 | Complete | SQLite backend with schema migrations |
| 7 | Complete | Normalized SQLite tables for customers, users, accounts, transactions, audit entries |
| 8 | Complete | Queryable audit and transaction history endpoints with SQLite indexes |
| 9 | Complete | Request IDs, structured tracing, login throttling |
| 10 | Complete | Bearer session tokens, token expiration, logout/revocation |

### Next Hardening Ideas

- Durable refreshable API sessions.
- Postgres support and production-grade migrations.
- Administrative unlock workflows for account lockouts.
- More complete OpenAPI documentation.
- Background jobs for scheduled fees, interest, and statement generation.

## فارسی

### معرفی

`bank` یک پروژه تمرینی اما جدی با Rust است که از یک برنامه ساده خط فرمان به یک backend بانکی چندلایه، تست‌پذیر و قابل توسعه تبدیل شده است. پروژه شامل منطق بانکی، هویت کاربران، سطح دسترسی، API، audit log، ذخیره‌سازی JSON و SQLite، session token، و سخت‌سازی عملیاتی API است.

این پروژه برای یادگیری و نمایش مهارت backend طراحی شده و نرم‌افزار مالی production محسوب نمی‌شود.

### قابلیت‌های اصلی

- مدل پول دقیق بر اساس cent، بدون استفاده از float.
- مدیریت چرخه عمر حساب: ساخت، ویرایش پروفایل، فعال‌سازی، غیرفعال‌سازی، بستن و حذف حساب.
- عملیات بانکی: واریز، برداشت، انتقال، کارمزد، سود، درخواست وام و پرداخت وام.
- تعریف مشتری، کاربر، نقش‌ها و permission برای customer، teller و admin.
- هش کردن رمز عبور با Argon2.
- احراز هویت API با Bearer token، انقضا و logout/revocation.
- audit log برای ثبت actor، action، outcome، target، پیام و زمان.
- ذخیره‌سازی snapshot با JSON و ذخیره‌سازی جدولی SQLite با migration.
- endpointهای قابل query برای audit و تاریخچه تراکنش‌ها.
- request id، tracing ساخت‌یافته و محدودسازی تلاش‌های ناموفق login.
- تست‌های واحد و API برای منطق بانکی، هویت، ذخیره‌سازی، migration و authorization.

### ساختار پروژه

```text
src/
  api.rs           REST API با Axum، session token، tracing و تست‌های API
  app.rs           ترکیب state اصلی برنامه
  audit.rs         مدل audit log
  bank.rs          منطق اصلی بانک و قوانین تجاری
  cli.rs           رابط خط فرمان تعاملی
  domain.rs        Money، Account، Transaction و typeهای دامنه
  identity.rs      Customer، User، Role، Permission و authentication
  sqlite_store.rs  migration و persistence نرمال‌شده SQLite
  storage.rs       snapshotهای JSON و خروجی statement
```

### اجرای پروژه

اجرای CLI:

```bash
cargo run
```

اجرای HTTP API:

```bash
cargo run -- serve 127.0.0.1:3000
```

ابتدا admin اولیه را بسازید:

```bash
curl -X POST http://127.0.0.1:3000/auth/bootstrap-admin \
  -H "content-type: application/json" \
  -d '{"user_id":1,"username":"admin","password":"correct-password"}'
```

سپس برای endpointهای محافظت‌شده از توکن برگشتی استفاده کنید:

```text
Authorization: Bearer <access_token>
```

بررسی کیفیت کد:

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

### فازهای انجام‌شده

| فاز | وضعیت | خلاصه |
| --- | --- | --- |
| 1 | کامل | پایه منطق بانکی، خطاهای typed، مدل دقیق Money و عملیات حساب |
| 2 | کامل | ذخیره‌سازی JSON، خروجی statement و تست persistence |
| 3 | کامل | مشتری، کاربر، نقش‌ها، permission و هش رمز با Argon2 |
| 4 | کامل | REST API با Axum و endpointهای محافظت‌شده |
| 5 | کامل | CI، شناسه تراکنش، timestamp و audit log |
| 6 | کامل | SQLite backend با schema migration |
| 7 | کامل | جدول‌های نرمال‌شده SQLite برای مشتری، کاربر، حساب، تراکنش و audit |
| 8 | کامل | query برای audit و transaction history همراه با indexهای SQLite |
| 9 | کامل | request id، tracing ساخت‌یافته و login throttling |
| 10 | کامل | Bearer session token، انقضا و logout/revocation |

### مسیرهای پیشنهادی بعدی

- sessionهای ماندگار و refreshable.
- پشتیبانی Postgres و migrationهای production-grade.
- workflow مدیریتی برای باز کردن lock کاربران.
- مستندات OpenAPI.
- jobهای زمان‌بندی‌شده برای کارمزد، سود و تولید statement.
