# posmini

POS ขนาดเล็กสำหรับร้านกาแฟ / ปาท่องโก๋ เขียนด้วย Rust (Axum + SQLite)

- หน้าขาย (แคชเชียร์) + จอลูกค้า + ลูกค้าสแกน QR สั่งเอง + เลขคิว + เรียกคิวด้วยเสียงภาษาไทย
- รับเงินสด (คีย์แพดคำนวณเงินทอน) และ QR พร้อมเพย์ ตรวจยอดเงินเข้าอัตโนมัติผ่าน API ของคุณเอง
- พิมพ์บิลลูกค้า (บัตรคิว + ใบเสร็จรวมใบเดียว) และใบออเดอร์สำหรับร้านเป็นภาพ ภาษาไทยถูกต้องทุกเครื่องพิมพ์
- คิวพิมพ์เรียงตามเลขคิว ไม่แซง ไม่หายเมื่อเครื่องพิมพ์ดับ
- หน้าสรุปยอดขาย วัน/เดือน/ปี

## โครงสร้าง

```
backend/    Rust API (+ ฝังหน้าเว็บไว้ใช้โหมดเครื่องเดียว)
frontend/   หน้าเว็บ static (public/) + Caddy ที่ proxy /api ไป backend
docker-compose.yml   รันเลย์เอาต์แบบคลาวด์ในเครื่อง
start.bat   รันบน Windows ที่ร้านพร้อม Cloudflare Tunnel (โหมดเครื่องเดียว)
```

## รันในเครื่อง (โหมดเครื่องเดียว)

```powershell
copy .env.example .env     # แก้ ADMIN_PASSWORD และค่า PROMPTPAY_*
cargo run --release        # http://localhost:8080/pos
```

หรือดับเบิลคลิก `start.bat` (รันเซิร์ฟเวอร์ + Cloudflare Tunnel) ในโหมดนี้เซิร์ฟเวอร์สั่งเครื่องพิมพ์ USB / Bluetooth ที่ต่อกับเครื่องเดียวกันได้โดยตรง

## Deploy บน Railway (แยก frontend / backend)

สร้างโปรเจกต์ Railway หนึ่งโปรเจกต์ มี 2 services จาก repo นี้ ทั้งสองใช้ **root ของ repo เป็น build context**
และชี้ Dockerfile ด้วยตัวแปร `RAILWAY_DOCKERFILE_PATH`

| Service    | `RAILWAY_DOCKERFILE_PATH` | Public domain | Volume |
|------------|---------------------------|---------------|--------|
| `backend`  | `backend/Dockerfile`      | ไม่ต้องเปิด (ใช้ private network) | `/data` |
| `frontend` | `frontend/Dockerfile`     | เปิด (Generate Domain) | – |

ผู้ใช้เข้าที่ domain ของ `frontend` เท่านั้น Caddy ส่ง `/api`, `/events`, `/img`, `/login` ต่อให้ backend ทาง
`backend.railway.internal` เบราว์เซอร์จึงเห็นเป็นเว็บเดียว (คุกกี้ล็อกอินและ live update ทำงานบน iPad/Safari ได้)

ตัวแปรของ `backend` (ใส่ใน Railway → Variables อย่าใส่ในโค้ด):

| ตัวแปร | ค่า |
|--------|-----|
| `PORT` | `8080` |
| `SHOP_SLUG`, `SHOP_NAME` | เช่น `main`, `ร้านของฉัน` |
| `ADMIN_USER`, `ADMIN_PASSWORD` | ผู้ใช้และรหัสผ่านแคชเชียร์ (ตั้งรหัสที่เดาไม่ได้) |
| `PAYMENT_API_BASE`, `PROMPTPAY_API_KEY`, `PROMPTPAY_ID` | ระบบตรวจยอดพร้อมเพย์ (ตั้งในหน้า ตั้งค่า > การรับเงิน แทนก็ได้) |

ตัวแปรของ `frontend`: `BACKEND_URL=http://backend.railway.internal:8080` (ค่าเริ่มต้นตรงกับชื่อ service `backend` อยู่แล้ว)

ฐานข้อมูล SQLite อยู่บน volume `/data` ของ backend (ต้องมี 1 replica) อย่าลืม backup ไฟล์ `/data/posmini.db` เป็นระยะ

### เครื่องพิมพ์เมื่อเซิร์ฟเวอร์อยู่บนคลาวด์

คลาวด์สั่งเครื่องพิมพ์ที่ร้านโดยตรงไม่ได้ จึงใช้ **สถานีพิมพ์ผ่านเบราว์เซอร์**:

1. หน้า จัดการ > เครื่องพิมพ์ เลือกวิธีเชื่อมต่อ **ผ่านเบราว์เซอร์** แล้วบันทึก
2. บนคอมพิวเตอร์ที่ต่อเครื่องพิมพ์ ใช้ **Chrome หรือ Edge** กด "เชื่อมต่อ Bluetooth / COM" (หรือ USB) แล้วเลือกเครื่องพิมพ์
3. เปิดหน้า **ขาย** ค้างไว้บนเครื่องนั้นตอนขาย หน้านั้นจะรับคิวพิมพ์จากเซิร์ฟเวอร์มาสั่งเครื่องพิมพ์เอง
   (iPad / Safari / Firefox เป็นสถานีพิมพ์ไม่ได้ แต่ใช้ขายและเรียกคิวได้ตามปกติ)

บิลเป็นภาพ เซิร์ฟเวอร์วาดเองด้วยฟอนต์ไทยใน image (`fonts-thai-tlwg`) ไม่ต้องตั้ง code page

ถ้าพิมพ์แล้วมีตัวอักษรมั่ว ๆ ปนกลางบิล แปลว่าเครื่องพิมพ์ (โดยเฉพาะ Bluetooth ราคาประหยัด) รับข้อมูลเร็วไป เพราะบัฟเฟอร์เล็กและบอกให้หยุดไม่ได้
ให้ตั้ง **ความเร็วส่งข้อมูล = ช้า** ในแท็บเครื่องพิมพ์ (ค่าเริ่มต้น) สถานีพิมพ์ส่งทีละ 256 ไบต์ตามความเร็วที่เลือก จับเวลาในตัว worker
จึงไม่ช้าลงเมื่อหน้าเว็บอยู่เบื้องหลัง และส่งไบต์ว่างปลุกเครื่องก่อนถ้าเครื่องเงียบไปนาน

## ติดตั้งเป็นแอป (มือถือ / แท็บเล็ต / คอมพิวเตอร์)

เว็บเป็น PWA: ติดตั้งแล้วเปิดเต็มจอจากหน้าจอหลักเหมือนแอปทั่วไป (ต้องเปิดผ่าน https เช่นโดเมน Railway)

| อุปกรณ์ | วิธีติดตั้ง |
|---------|-------------|
| Android (Chrome) | แถบ "ติดตั้งเป็นแอป" จะขึ้นเองที่หน้าเข้าสู่ระบบ กด **ติดตั้ง** (หรือเมนู ⋮ > ติดตั้งแอป) |
| iPhone / iPad (Safari) | แตะปุ่มแชร์ แล้วเลือก **เพิ่มลงในหน้าจอโฮม** (iOS ไม่มีปุ่มติดตั้งให้เว็บเรียก) |
| Windows / Mac (Chrome, Edge) | ไอคอนติดตั้งที่ปลายแถบที่อยู่ หรือแถบ "ติดตั้งเป็นแอป" ในหน้าเว็บ |

ชื่อใต้ไอคอนคือชื่อร้านจาก `SHOP_NAME` (`/manifest.webmanifest` สร้างจาก backend) กดไอคอนค้างบน Android ได้ทางลัด ขายหน้าร้าน / จอลูกค้า / สรุปยอด
ถ้าไม่มีอินเทอร์เน็ต แอปจะขึ้นหน้า "ไม่มีสัญญาณอินเทอร์เน็ต" (การขายต้องใช้เซิร์ฟเวอร์) ส่วนข้อมูลสด `/api` และ `/events` ไม่ถูกแคช
จึงไม่มีคิวหรือยอดเก่าค้าง ไอคอนสร้างจาก `frontend/tools/make_icons.py`

## ทดสอบ

| ชุดทดสอบ | คำสั่ง | ครอบคลุม |
|----------|--------|----------|
| Rust (unit + HTTP end-to-end) | `cargo fmt --check; cargo clippy --all-targets -- -D warnings; cargo test` | ล็อกอิน, ออเดอร์เงินสด/เงินทอน, รูปแบบเลขคิว, คิวพิมพ์เรียงลำดับและสถานีพิมพ์ผ่านเบราว์เซอร์, การเรียกคิวแซง, โหมด API-only |
| Frontend (node:test) | `node --test "frontend/tests/*.test.js"` | เครื่องคิดเลข, ทุกหน้าโหลดไฟล์ที่มีจริง, ไม่มี placeholder ค้าง, เส้นทาง Caddy, backend ฝังไฟล์ static ครบ, ไอคอน/manifest ของแอป, service worker (จำลองสภาพแวดล้อมแล้วเช็กว่าไม่แคช API และออฟไลน์แล้วขึ้นหน้าสำรอง) |
| Smoke test (ระบบที่รันอยู่) | `bash scripts/smoke.sh <url>` | หน้าเว็บ, proxy ไป backend, SSE, API ถูกป้องกัน (ใส่ `SMOKE_ADMIN_PASSWORD` เพื่อทดสอบหลังล็อกอิน และ `SMOKE_WRITE=1` เพื่อบันทึกการขายทดสอบ 1 รายการ — ใช้กับระบบทดสอบเท่านั้น) |

## CI / CD

```
git push → GitHub Actions (CI) → ผ่าน → Railway deploy backend + frontend → smoke test ระบบจริง
```

- `.github/workflows/ci.yml` รันทุกครั้งที่ push เข้า `main` และ pull request: ชุดทดสอบ Rust, ชุดทดสอบ frontend, build Docker ทั้งสอง image, ตรวจ Caddyfile, แล้วยก backend + frontend ขึ้นต่อกันเหมือนบน Railway และรัน smoke test พร้อมขายจริง 1 รายการ
- Railway ตั้ง **Wait for CI** ไว้ ถ้า CI แดง commit นั้นจะไม่ถูก deploy
- `.github/workflows/post-deploy.yml` รัน smoke test (อ่านอย่างเดียว) กับเว็บจริงหลัง Railway deploy สำเร็จ ตั้งที่อยู่ครั้งเดียวด้วย
  `gh variable set SMOKE_URL --body https://<โดเมน frontend>` หรือสั่งรันเองจากแท็บ Actions
- การตั้งค่า Railway อยู่ในโค้ดที่ `.railway/railway.ts` (Infrastructure as Code ของ Railway): สอง service, Dockerfile ของแต่ละตัว,
  healthcheck `/health`, restart เมื่อล้ม, watch paths (แก้เฉพาะหน้าเว็บไม่ทำให้ build backend ใหม่), Wait for CI, volume `/data`
  ตัวแปรลับ (`ADMIN_PASSWORD` ฯลฯ) **ไม่อยู่ในไฟล์** ตั้งใน Railway เอง

```powershell
npm install                 # ติดตั้ง SDK ของ Railway (ครั้งแรก)
npm run railway:plan        # ดูว่าจะเปลี่ยนอะไรบน Railway
npm run railway:apply       # ใช้การเปลี่ยนแปลง
```

> Windows: ถ้าติดตั้ง Railway CLI ผ่าน npm และเจอ error "requires Railway CLI 5.42.1 or newer" ให้ตั้งตัวแปร `_` ชี้ไปที่ตัว `.exe`
> ก่อนรัน เช่น `$env:_ = "$env:APPDATA\npm\node_modules\@railway\cli\bin\railway.exe"`
