# Мостовая схема VPN: TrustTunnel → Российский VPS → VLESS+REALITY → Зарубежный VPS

Схема маршрутизации:

Клиент (телефон/ПК) → **TrustTunnel** → **Российский VPS (Мост)** → **VLESS+REALITY (Xray)** → **Зарубежный VPS (Цель)** → Интернет

Инструкция рассчитана на специалиста среднего уровня, знакомого с Linux, systemd, SSH и базовыми сетевыми концепциями (IP-адресация, маршруты, таблицы маршрутизации).

> **Важно. Юридический дисклеймер**  
> Использование VPN/прокси может регулироваться законодательством вашей страны. Убедитесь, что вы не нарушаете местные законы и политику провайдеров.

---

## 1. Подготовка инфраструктуры

### 1.1. Выбор провайдеров и локаций

Рекомендуемая топология:

- **Зарубежный VPS (Цель)**
  - Локация: ЕС (Германия/Нидерланды), США, или любая стабильная юрисдикция с нейтральной политикой.
  - Требования: 
    - 1 vCPU, 1–2 ГБ RAM, 10+ ГБ диска
    - Публичный IPv4 (желательно белый, без блок-листов)
  - Примеры провайдеров: Hetzner, Contabo, Vultr, OVH, DigitalOcean и т.п.

- **Российский VPS (Мост)**
  - Локация: РФ (любой регион)
  - Требования:
    - 1 vCPU, 1 ГБ RAM, 10+ ГБ диска
    - Публичный IPv4, стабильный канал к вашим клиентам
  - К нему будет подключаться **TrustTunnel** с клиентских устройств.

### 1.2. Операционная система

На обоих серверах рекомендуется использовать:

- **Ubuntu 22.04 LTS** (минимальный образ)

Все команды далее приведены под root (или через `sudo`).

```bash
# Если вы заходите под обычным пользователем
sudo -i
```

### 1.3. Домен и DNS

Для REALITY домен **не обязателен**, но для большей гибкости и маскировки полезно иметь домен, который указывает на зарубежный VPS или хотя бы может использоваться как «жертва» (victim-domain) для имитации реального трафика.

**Два типа доменов в контексте REALITY и мостовой схемы:**

1. **Victim-домен (Server Name / SNI)**
   - Домен, который вы будете имитировать (например, `www.cloudflare.com` или `www.bing.com`).  
   - Этот домен **не обязан** указывать на ваш сервер.
   - Главное — чтобы он был:
     - популярным,
     - с поддержкой TLS 1.3 и ESNI/QUIC/TLS1.3,
     - без блокировок.

2. **Связанный домен**
   - Ваш собственный домен, который:
     - может использоваться для любых веб-сервисов,
     - **критически важен** для работы TLS/Let’s Encrypt на **Российском VPS**, через который клиенты подключаются по TrustTunnel.
   - Именно к Российскому VPS будут подключаться клиенты (мобильные/десктопные), поэтому наличие домена, указывающего на его IP, упрощает:
     - выпуск и автоматическое продление сертификатов (Let’s Encrypt, Certbot),
     - работу TrustTunnel в режиме TLS,
     - правдоподобную маскировку под обычный HTTPS-сервис.

> **Замечание:** В конфигурации REALITY мы будем использовать только victim-домен (SNI) — он нужен для имитации легитимного TLS-трафика на Зарубежном VPS. А собственный домен — для TLS на Российском VPS (TrustTunnel, панели, вспомогательные сервисы).

#### 1.3.1. Практика: регистрация дешёвого домена на Reg.ru и настройка DNS для Российского VPS

На примере Reg.ru (подход у других регистраторов аналогичен):

1. **Выбор дешёвого домена**
   - Зайдите на сайт регистратора (например, `reg.ru`).
   - Введите желаемое имя домена в поиске.
   - Выберите одну из дешёвых зон:
     - `.ru` или `.рф` — как правило ~200–250 руб/год (часто бывают акции ещё дешевле).
   - Регистрируйте домен на физическое лицо, стандартная процедура.

2. **Привязка домена к Российскому VPS**
   - В панели Reg.ru откройте управление доменом → раздел DNS/«Управление зоной DNS».
   - Создайте/отредактируйте A-запись:
     - Имя (host):  
       - `@` — для корня домена (`example.ru`),  
       - при желании отдельный поддомен, например `vpn.example.ru`.
     - Тип: `A`
     - Значение (IP): **публичный IPv4 Российского VPS**
   - Сохраните изменения.  
     DNS-обновление может занять от нескольких минут до 1–2 часов (обычно быстрее).

3. **Выпуск SSL-сертификата для Российского VPS через Let’s Encrypt (Certbot)**

   Почему так:
   - **Let’s Encrypt (через Certbot)**:
     - бесплатен,
     - признан стандартом индустрии,
     - автоматизирует выпуск и продление сертификатов,
     - не требует ручной оплаты/продления, в отличие от платных сертификатов (AlphaSSL и т.п.).
   - **Платные сертификаты (AlphaSSL и др.)** не дают дополнительной защиты для данной схемы, а лишь создают лишние расходы и сложность.

   Базовый пример установки Certbot на Российском VPS (если вы захотите выпустить сертификат отдельно от мастера TrustTunnel):

   ```bash
   apt update
   apt install -y certbot
   ```

   Выпуск сертификата для домена (пример для `vpn.example.ru`, режим standalone, без уже работающего веб-сервера):

   ```bash
   certbot certonly --standalone -d vpn.example.ru
   ```

   После успешного выпуска сертификаты обычно лежат в:

   ```text
   /etc/letsencrypt/live/vpn.example.ru/fullchain.pem
   /etc/letsencrypt/live/vpn.example.ru/privkey.pem
   ```

   Эти пути можно:
   - напрямую указывать в `vpn.toml` TrustTunnel как `tls_cert` / `tls_key`,  
   - либо TrustTunnel может сам интегрироваться с Let’s Encrypt через свой мастер (`setup_wizard`) — это предпочтительный путь, так как он сразу настраивает автопродление.

---

### 1.4. Базовая подготовка серверов

На **обоих VPS**:

```bash
apt update && apt upgrade -y
apt install -y curl wget jq nano git
```

Проверьте, что включен `ufw` или другой файрволл, либо временно отключите, чтобы не мешать отладке (позже можно ужесточить):

```bash
ufw status
# при необходимости
ufw disable
```

---

## 2. Настройка Зарубежного VPS (Цель): Xray + VLESS+REALITY

### 2.1. Установка Xray (оф. скрипт)

На **Зарубежном VPS**:

```bash
bash <(curl -L https://github.com/XTLS/Xray-install/raw/main/install-release.sh) install
```

Проверка версии и статуса:

```bash
/usr/local/bin/xray -version
systemctl status xray --no-pager
```

Если сервис не запущен, включаем автозапуск:

```bash
systemctl enable xray
systemctl start xray
```

### 2.2. Подготовка параметров для REALITY

#### 2.2.1. Генерация UUID и ключей REALITY

```bash
# UUID для VLESS
uuid=$(cat /proc/sys/kernel/random/uuid)
echo "UUID: $uuid"

# Генерация ключей REALITY
/usr/local/bin/xray x25519
```

Вывод будет вида:

```text
Private key: XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
Public key:  YYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYY
```

Сохраните:

- `UUID`
- `Private key`
- `Public key`

#### 2.2.2. Выбор victim-домена

Примеры подходящих victim-доменов:

- `www.cloudflare.com`
- `www.bing.com`
- `www.cloudflarestatus.com`
- `www.microsoft.com`
- `www.yahoo.com`

Мы будем использовать, например, `www.cloudflare.com`.

### 2.3. Пример конфига Xray (REALITY + VLESS)

Создаём/редактируем `/usr/local/etc/xray/config.json`:

```bash
nano /usr/local/etc/xray/config.json
```

Вставьте конфигурацию (замените `UUID_HERE`, `REALITY_PRIVATE_KEY`, `REALITY_PUBLIC_KEY` при необходимости, `serverName` — victim-домен):

```json
{
  "log": {
    "loglevel": "warning"
  },
  "inbounds": [
    {
      "port": 443,
      "protocol": "vless",
      "settings": {
        "clients": [
          {
            "id": "UUID_HERE",
            "flow": "xtls-rprx-vision"
          }
        ],
        "decryption": "none",
        "fallbacks": []
      },
      "streamSettings": {
        "network": "tcp",
        "security": "reality",
        "realitySettings": {
          "show": false,
          "dest": "www.cloudflare.com:443",
          "xver": 0,
          "serverNames": [
            "www.cloudflare.com"
          ],
          "privateKey": "REALITY_PRIVATE_KEY",
          "shortIds": [
            "abcd1234"
          ]
        }
      }
    }
  ],
  "outbounds": [
    {
      "protocol": "freedom",
      "tag": "direct"
    },
    {
      "protocol": "blackhole",
      "tag": "blocked"
    }
  ]
}
```

**Комментарии к полям:**

- `port`: внешний порт сервера (обычно `443`, имитирует HTTPS).
- `id`: ваш `UUID` клиента.
- `flow`: `xtls-rprx-vision` — рекомендованный режим для REALITY.
- `serverNames`: victim-домен, который Xray будет имитировать.
- `dest`: тот же домен на 443, куда Xray подделывает трафик TLS.
- `privateKey`: приватный ключ REALITY (из `xray x25519`).
- `shortIds`: произвольные короткие ID (до 8 байт, записываются в hex; например, `"abcd1234"` – это 4 байта). Можно задать несколько значений.

Подставляем реальные значения:

```bash
# пример быстрой замены значений sed-ом (по желанию)
sed -i "s/UUID_HERE/$uuid/" /usr/local/etc/xray/config.json
```

### 2.3.1. Что такое `shortIds` в REALITY и зачем они нужны

В REALITY поле `shortIds` — это набор **коротких идентификаторов** (short IDs), которые:

- записываются в **hex-формате**;
- имеют длину **до 8 байт** (то есть до 16 hex-символов);
- используются сервером для **быстрого отличия легитимных клиентов от активного зондирования** (active probing) со стороны систем фильтрации и сканеров.

Основные функции `shortIds`:

1. **Фильтрация активного зондирования**
   - DPI/сканеры пытаются «нащупать» нестандартные сервисы, устанавливая полуслучайные TLS-соединения.
   - REALITY ожидает от клиента корректный `shortId`; если он не совпадает ни с одним из настроенных значений:
     - соединение может быть немедленно отброшено или обработано как обычный HTTPS-трафик,
     - реальный маршрут VLESS не «подсвечивается» для сканера.

2. **Гибкое управление доступом**
   - Вы можете настроить **несколько значений** `shortIds`:
     - разные группы пользователей (например, для отдельных команд или серверов),
     - временные/одноразовые `shortIds` для ограниченного доступа.
   - При компрометации одного `shortId` его можно просто удалить из списка без смены UUID и ключей.

3. **Усложнение статистического анализа**
   - Наличие нескольких `shortIds` и их распределение по пользователям/периодам:
     - затрудняет построение стабильного сигнатурного профиля соединений,
     - уменьшает вероятность того, что DPI сможет «привязать» ваш сервер к одному характерному паттерну рукопожатия.

**Практические рекомендации:**

- Для каждого пользователя/группы можно задать **уникальный shortId**, например:

  ```json
  "shortIds": [
    "abcd1234",
    "1f2e3d4c",
    "11223344"
  ]
  ```

- Не используйте тривиальные значения вроде всех нулей.
- Периодически **ротуйте** (обновляйте) `shortIds`, если есть риск их утечки.

---

### 2.4. Перезапуск Xray и проверка

```bash
systemctl restart xray
systemctl status xray --no-pager
journalctl -u xray -f
```

Убедитесь, что нет ошибок парсинга JSON и Xray слушает порт 443:

```bash
ss -tulpen | grep 443
```

Пример результата:

```text
tcp   LISTEN 0      4096         0.0.0.0:443    0.0.0.0:*    users:(("xray",pid=1234,fd=3))
```

Сохраните параметры для будущих клиентов (Российский VPS и прямые клиенты):

- IP Зарубежного VPS: `X.X.X.X`
- Порт: `443`
- UUID: `<ваш_UUID>`
- Public key: `REALITY_PUBLIC_KEY`
- ShortId: `abcd1234`
- ServerName (SNI): `www.cloudflare.com`

---

### 2.5. Почему для моста выбран Xray (VLESS+REALITY), а не Shadowsocks

В современных условиях блокировок ключевую роль играют **DPI-системы (Deep Packet Inspection)**, которые:

- анализируют:
  - заголовки и рукопожатия протоколов,
  - статистику пакетов (длины, интервалы, энтропию),
  - аномалии в поведении соединений;
- строят **поведенческие и статистические сигнатуры** трафика.

**Shadowsocks (даже с плагинами-обфускаторами):**

- Использует относительно простой формат шифрованного трафика.
- Характеризуется:
  - высокой и довольно стабильной энтропией полезной нагрузки,
  - узнаваемым распределением длины пакетов и рукопожатий.
- Даже при использовании плагинов (obfs, v2ray-plugin, simple-obfs и т.п.):
  - часто эмулирует «похожий на HTTPS» трафик лишь частично,
  - DPI могут отличать такой «квазисекьюрный» трафик от **реального HTTPS** по:
    - аномальным версиям TLS/шифросуитам,
    - отсутствию/нестандартности расширений (ALPN, SNI),
    - статистике поведения соединения (паттерны, длина/энтропия пакетов, фаза рукопожатия).

**Xray (VLESS+REALITY):**

- **Полная имитация TLS 1.3**: Трафик REALITY для внешнего наблюдателя выглядит как легитимная сессия TLS 1.3 к популярному доверенному домену (например, Microsoft или Cloudflare).
- **Steal Mode**: REALITY «крадет» параметры рукопожатия у реального сервера, что делает невозможным его детектирование через active probing (активное зондирование).
- **Отсутствие характерных сигнатур**: В отличие от Shadowsocks, REALITY не имеет специфических статистических аномалий, которые позволяют DPI-системам выделить его из общего потока HTTPS-трафика.

Таким образом, использование VLESS+REALITY в качестве «внешнего» контура между серверами обеспечивает значительно более высокий уровень защиты от блокировок по сравнению с классическими реализациями Shadowsocks.

---

## 3. Настройка Российского VPS (Мост)

Задачи Российского VPS:

1. Принимать подключения от клиентов через **TrustTunnel** (VPN/Proxy).
2. Поднимать **Xray-клиент**, который использует VLESS+REALITY для соединения с зарубежным сервером.
3. Пробрасывать трафик из подсети TrustTunnel через **tun2socks** → Xray → Зарубежный VPS.

### 3.1. Установка TrustTunnel Endpoint

Актуальная версия TrustTunnel ставится и настраивается через официальный скрипт и интерактивный мастер конфигурации. Ниже — адаптированный под нашу схему базовый сценарий.

#### 3.1.1. Установка через официальный скрипт

На **Российском VPS**:

```bash
curl -fsSL https://raw.githubusercontent.com/TrustTunnel/TrustTunnel/refs/heads/master/scripts/install.sh | sh -s -
```

Ключевые моменты:

- Скрипт устанавливает TrustTunnel по умолчанию в каталог:

  ```text
  /opt/trusttunnel
  ```

- Бинарники, как правило, попадают в:

  ```text
  /opt/trusttunnel/bin
  ```

  и добавляются в `PATH` (или вам будет предложено это сделать).
- Создаются базовые каталоги для конфигурации и сертификатов:

  ```text
  /opt/trusttunnel/config
  /opt/trusttunnel/certs
  ```

Проверьте, что бинарник доступен:

```bash
/opt/trusttunnel/bin/trusttunnel_endpoint --help
```

(Название бинарника может отличаться в зависимости от релиза, в документации он обозначается как `trusttunnel_endpoint`.)

#### 3.1.2. Базовая настройка через setup_wizard

Для первой инициализации конечной точки (Endpoint) используем встроенный мастер:

```bash
cd /opt/trusttunnel
./bin/setup_wizard endpoint
```

Типичный диалог мастера (примерно, фактические вопросы могут отличаться):

- Режим работы: `endpoint`
- Внешний адрес/домен Endpoint:
  - `RUSSIAN_VPS_IP` или домен, указывающий на Российский VPS.
- VPN-подсеть:
  - `10.10.0.0/24`
- IP Endpoint внутри VPN:
  - `10.10.0.1`
- Имя VPN-интерфейса:
  - `tt0`  
    (это важно: далее в инструкции мы предполагаем интерфейс `tt0` и подсеть `10.10.0.0/24`).
- Порт для VPN:
  - например, `51820` (UDP).
- Режим TLS для внешнего слушающего сокета:
  - `Let's Encrypt` / `Self-signed` / `Existing certificate`

Мастер:

- создаст TOML-конфигурации в `/opt/trusttunnel/config`:
  - `vpn.toml` — параметры VPN-интерфейса Endpoint;
  - `hosts.toml` — описание клиентов/хостов;
- сгенерирует/разместит TLS-сертификаты в `/opt/trusttunnel/certs` в зависимости от выбранного режима.

#### 3.1.3. Настройка TLS-сертификатов

В мастерe `setup_wizard` можно выбрать один из режимов:

1. **Let's Encrypt**
   - Мастер запросит домен, выполнит HTTP- или DNS-валидацию (в зависимости от реализации) и поместит выданные сертификаты в `/opt/trusttunnel/certs`.
   - В `vpn.toml` будут указаны пути к сертификату и ключу, например:

     ```toml
     tls_cert = "/opt/trusttunnel/certs/fullchain.pem"
     tls_key  = "/opt/trusttunnel/certs/privkey.pem"
     ```

2. **Self-signed**
   - TrustTunnel сгенерирует самоподписанный сертификат.
   - Подходит, если клиенты TrustTunnel готовы доверять этому сертификату (либо сертификат будет установлен в доверенные на устройствах).

3. **Existing**
   - Указываете пути к уже имеющимся сертификатам (например, от стороннего ACME-клиента).
   - Мастер пропишет эти пути в `vpn.toml`.

При необходимости вы можете позже отредактировать пути к сертификатам вручную в `vpn.toml`.

#### 3.1.4. TOML-конфигурации: `vpn.toml` и `hosts.toml`

После работы `setup_wizard` проверьте содержимое:

```bash
nano /opt/trusttunnel/config/vpn.toml
```

Пример (обобщённый, синтаксис TOML):

```toml
# /opt/trusttunnel/config/vpn.toml

[vpn]
interface_name = "tt0"
address        = "10.10.0.1/24"
listen_port    = 51820
protocol       = "wireguard"      # пример, зависит от фактического протокола реализации
transport      = "tls"

[tls]
mode     = "letsencrypt"          # "letsencrypt" | "self-signed" | "existing"
domain   = "vpn.example.com"      # домен Endpoint (для LE)
cert     = "/opt/trusttunnel/certs/fullchain.pem"
key      = "/opt/trusttunnel/certs/privkey.pem"

[logging]
level = "info"
path  = "/opt/trusttunnel/logs/endpoint.log"
```

Файл `hosts.toml` описывает клиентов/хостов, которым выдаются адреса из подсети `10.10.0.0/24`. Откройте:

```bash
nano /opt/trusttunnel/config/hosts.toml
```

Типичный пример:

```toml
# /opt/trusttunnel/config/hosts.toml

[[clients]]
name       = "client1"
address    = "10.10.0.2/32"
public_key = "CLIENT1_PUBLIC_KEY"

[[clients]]
name       = "client2"
address    = "10.10.0.3/32"
public_key = "CLIENT2_PUBLIC_KEY"
```

> **Важно:**  
> - Подсеть `10.10.0.0/24` и интерфейс `tt0` должны соответствовать параметрам, которые мы далее используем в разделах 3.4 и 4.x.  
> - Если вы меняете подсеть или имя интерфейса, не забудьте внести такие же изменения в правила маршрутизации (раздел 3.4.4) и клиентские конфиги.

#### 3.1.5. Установка systemd-сервиса из шаблона

Официальная поставка TrustTunnel включает systemd-шаблон сервиса, например:

```text
/opt/trusttunnel/systemd/trusttunnel.service.template
```

Скопируйте его в `/etc/systemd/system/` и при необходимости скорректируйте:

```bash
cp /opt/trusttunnel/systemd/trusttunnel.service.template /etc/systemd/system/trusttunnel.service
nano /etc/systemd/system/trusttunnel.service
```

Пример того, как может выглядеть unit (обобщённо):

```ini
[Unit]
Description=TrustTunnel Endpoint
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/trusttunnel
ExecStart=/opt/trusttunnel/bin/trusttunnel_endpoint \
  --config /opt/trusttunnel/config/vpn.toml \
  --hosts  /opt/trusttunnel/config/hosts.toml
Restart=on-failure
User=root    # или специализированный пользователь, если предусмотрен установщиком
Group=root

[Install]
WantedBy=multi-user.target
```

Активируем сервис:

```bash
systemctl daemon-reload
systemctl enable trusttunnel
systemctl start trusttunnel
systemctl status trusttunnel --no-pager
```

#### 3.1.6. Генерация клиентских конфигов

После настройки Endpoint вы можете сгенерировать конфигурацию для клиента (например, в формате TOML или URI):

```bash
cd /opt/trusttunnel
./bin/trusttunnel_endpoint \
  --config /opt/trusttunnel/config/vpn.toml \
  --hosts  /opt/trusttunnel/config/hosts.toml \
  -c client1 \
  -a RUSSIAN_VPS_IP:51820 \
  --format toml
```

Это выведет параметры, которые нужно использовать на стороне клиента.

#### 3.1.7. Смена Self-signed сертификата на Let's Encrypt (для существующей установки)

Если TrustTunnel уже установлен с самоподписанным сертификатом, выполните следующие шаги для перехода на Let's Encrypt:

1. **Установка Certbot** (если ещё не установлен):
   ```bash
   apt update
   apt install -y certbot
   ```

2. **Выпуск сертификата** (убедитесь, что порт 80 открыт):
   ```bash
   certbot certonly --standalone -d way.admishakov.ru
   ```
   *Замените `way.admishakov.ru` на ваш актуальный поддомен.*

3. **Обновление конфигурации**:

   **Вариант А: Через интерактивный мастер (рекомендуется)**
   ```bash
   cd /opt/trusttunnel
   ./setup_wizard
   ```
   В мастере выберите роль `Endpoint`, затем `Existing Certificate` и укажите пути:
   - Certificate: `/etc/letsencrypt/live/way.admishakov.ru/fullchain.pem`
   - Key: `/etc/letsencrypt/live/way.admishakov.ru/privkey.pem`
   - Domain: `way.admishakov.ru`

   **Вариант Б: Ручное редактирование `vpn.toml`**
   Откройте файл `/opt/trusttunnel/vpn.toml` (или `/opt/trusttunnel/config/vpn.toml`) и приведите секцию `[tls]` к виду:
   ```toml
   [tls]
   mode   = "existing"
   domain = "way.admishakov.ru"
   cert   = "/etc/letsencrypt/live/way.admishakov.ru/fullchain.pem"
   key    = "/etc/letsencrypt/live/way.admishakov.ru/privkey.pem"
   ```

4. **Перезапуск сервиса**:
   ```bash
   systemctl restart trusttunnel
   ```

#### 3.1.8. Проверка корректности установки SSL

Для проверки того, что сервер «отдает» правильный сертификат, используйте следующие команды:

**Проверка через домен (если DNS обновился):**
```bash
openssl s_client -connect way.admishakov.ru:443 -servername way.admishakov.ru | openssl x509 -noout -text | grep -E "Subject:|After"
```

**Проверка напрямую через IP (если DNS еще не работает):**
```bash
openssl s_client -connect 95.140.159.63:443 -servername way.admishakov.ru | openssl x509 -noout -text | grep -E "Subject:|After"
```

*Ожидаемый результат: `Subject: CN = way.admishakov.ru` и актуальная дата окончания.*

5. **Настройка автопродления**:
   Чтобы TrustTunnel подхватывал обновленные сертификаты, добавьте в `hook` Certbot перезапуск сервиса:
   Создайте файл `/etc/letsencrypt/renewal-hooks/deploy/trusttunnel.sh`:
   ```bash
   #!/bash
   systemctl restart trusttunnel
   ```
   Сделайте его исполняемым: `chmod +x /etc/letsencrypt/renewal-hooks/deploy/trusttunnel.sh`

---

### 3.2. Установка Xray-клиента на Российском VPS

```bash
bash <(curl -L https://github.com/XTLS/Xray-install/raw/main/install-release.sh) install
```

Проверяем установку:

```bash
/usr/local/bin/xray -version
```

### 3.3. Конфиг Xray (клиент VLESS+REALITY)

Создаём конфиг `/usr/local/etc/xray/client.json`:

```bash
nano /usr/local/etc/xray/client.json
```

Вставьте пример конфига (замените `<...>` своими значениями):

```json
{
  "log": {
    "loglevel": "warning"
  },
  "inbounds": [
    {
      "tag": "socks-in",
      "port": 1080,
      "listen": "127.0.0.1",
      "protocol": "socks",
      "settings": {
        "udp": true,
        "auth": "noauth"
      }
    },
    {
      "tag": "tun-in",
      "protocol": "dokodemo-door",
      "listen": "127.0.0.1",
      "port": 12345,
      "settings": {
        "network": "tcp,udp",
        "followRedirect": true
      },
      "streamSettings": {
        "sockopt": {
          "tproxy": "tproxy"
        }
      }
    }
  ],
  "outbounds": [
    {
      "tag": "vless-out",
      "protocol": "vless",
      "settings": {
        "vnext": [
          {
            "address": "FOREIGN_VPS_IP",
            "port": 443,
            "users": [
              {
                "id": "UUID_HERE",
                "encryption": "none",
                "flow": "xtls-rprx-vision"
              }
            ]
          }
        ]
      },
      "streamSettings": {
        "network": "tcp",
        "security": "reality",
        "realitySettings": {
          "serverName": "www.cloudflare.com",
          "fingerprint": "chrome",
          "publicKey": "REALITY_PUBLIC_KEY",
          "shortId": "abcd1234",
          "spiderX": "/"
        }
      }
    },
    {
      "protocol": "freedom",
      "tag": "direct"
    },
    {
      "protocol": "blackhole",
      "tag": "blocked"
    }
  ],
  "routing": {
    "domainStrategy": "AsIs",
    "rules": [
      {
        "type": "field",
        "inboundTag": ["socks-in", "tun-in"],
        "outboundTag": "vless-out"
      }
    ]
  }
}
```

Создаём unit-файл для клиентского Xray:

```bash
nano /etc/systemd/system/xray-client.service
```

```ini
[Unit]
Description=Xray Client (VLESS REALITY)
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/xray run -c /usr/local/etc/xray/client.json
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Активируем:

```bash
systemctl daemon-reload
systemctl enable xray-client
systemctl start xray-client
```

### 3.4. Настройка сетевого моста через tun2socks

Цель: весь трафик клиентов, приходящий в подсеть TrustTunnel (`10.10.0.0/24`), перенаправить через `tun2socks` → `Xray` → зарубежный сервер.

#### 3.4.1. Установка tun2socks

Используем `gost`:

```bash
cd /usr/local/bin
wget -O gost https://github.com/go-gost/gost/releases/latest/download/gost_linux_amd64
chmod +x gost
```

#### 3.4.2. Запуск tun2socks через gost (systemd)

Создаём виртуальный интерфейс `tun0` (подсеть `10.20.0.0/24`) и связываем его с SOCKS5 Xray:

```bash
nano /etc/systemd/system/tun2socks.service
```

```ini
[Unit]
Description=Tun2Socks via gost
After=network.target xray-client.service trusttunnel.service

[Service]
Type=simple
ExecStartPre=/sbin/ip tuntap add mode tun dev tun0
ExecStartPre=/sbin/ip addr add 10.20.0.1/24 dev tun0
ExecStartPre=/sbin/ip link set tun0 up
ExecStart=/usr/local/bin/gost -L tun://:0?net=10.20.0.2/24&gw=10.20.0.1 -F socks5://127.0.0.1:1080
ExecStopPost=/sbin/ip link set tun0 down
ExecStopPost=/sbin/ip tuntap del mode tun dev tun0
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Активируем:

```bash
systemctl daemon-reload
systemctl enable tun2socks
systemctl start tun2socks
```

#### 3.4.3. Маршрутизация трафика из подсети TrustTunnel (10.10.0.0/24)

1. Добавляем таблицу в `/etc/iproute2/rt_tables`:

```bash
echo "200 ttroute" >> /etc/iproute2/rt_tables
```

2. Настраиваем правила через systemd-сервис:

```bash
nano /etc/systemd/system/ttroute-setup.service
```

```ini
[Unit]
Description=Routing for TrustTunnel Subnet
After=network-online.target trusttunnel.service tun2socks.service
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/sbin/ip route add default dev tun0 table ttroute
ExecStart=/sbin/ip rule add from 10.10.0.0/24 lookup ttroute
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
```

Активируем:

```bash
systemctl daemon-reload
systemctl enable ttroute-setup
systemctl start ttroute-setup
```

---

## 4. Настройка клиента (телефон/ПК)

Клиент должен установить соединение с Российским VPS через TrustTunnel.

1. **Установка клиента:** Используйте официальный скрипт для Linux/macOS или мобильные приложения (AdGuard VPN или TrustTunnel-совместимые).
   ```bash
   curl -fsSL https://raw.githubusercontent.com/TrustTunnel/TrustTunnelClient/refs/heads/master/scripts/install.sh | sh -s -
   ```
2. **Настройка:** Используйте `setup_wizard` на стороне клиента, указав конфиг, сгенерированный на Endpoint (шаг 3.1.6).
   ```bash
   cd /opt/trusttunnel_client
   ./setup_wizard --mode non-interactive --endpoint_config client1.toml --settings trusttunnel_client.toml
   ```
3. **Запуск:**
   ```bash
   sudo ./trusttunnel_client -c trusttunnel_client.toml
   ```

---

## 5. Проверка работоспособности

1. Клиент подключается к Российскому VPS.
2. IP клиента — `10.10.0.X`.
3. Внешний IP (проверка через `curl ifconfig.me`) — **IP Зарубежного VPS**.

---

## 6. Резюме

Схема обеспечивает высокую скрытность (REALITY имитирует HTTPS) и низкую задержку внутри РФ (прямой канал к Российскому VPS). TrustTunnel выступает в роли надежного транспорта, а Xray обеспечивает обход блокировок на внешнем контуре.
