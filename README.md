# rc-talk

Плагин общения для RustCloud: личные и групповые чаты между пользователями облака, звонки через
LiveKit (когда ядро даст `permissions.connect` и `permissions.media`).

Устройство — [docs/design.md](docs/design.md). Что нужно от ядра —
[core-proposal.md](core-proposal.md) и ответ разработчиков [core-response.md](core-response.md).

## Сборка

```sh
rustup target add wasm32-wasip2
cargo test                                         # правила и форматы, на хосте
cargo build --release --target wasm32-wasip2
mkdir -p dist/rc-talk && cp manifest.toml dist/rc-talk/
cp target/wasm32-wasip2/release/rc_talk.wasm dist/rc-talk/app.wasm
(cd dist && zip -r rc-talk.zip rc-talk)
```

`wit/app.wit` — копия контракта из `rustcloud/crates/plugin-api/wit/app.wit` (0.2.18).

## Установка

Администрирование → Плагины → установить `rc-talk.zip`, включить. Пока облако не умеет доставку между
пользователями, в настройках плагина включите «Временная доставка через хранилище плагина»: без неё
пользователи не смогут писать друг другу (вкладка «Состояние» об этом предупреждает). Эта настройка —
только для стенда: сборку с ней в каталог не выпускать.

Для звонков (позже) — адрес, ключ и секрет LiveKit в тех же настройках; адрес API LiveKit для
сервера — чтобы убирать из звонка исключённых из группы.
