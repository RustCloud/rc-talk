# rc-talk

Плагин общения для RustCloud: личные и групповые чаты между пользователями облака, звонки через
LiveKit (когда ядро даст `permissions.media`). Нужен RustCloud с API плагинов 0.2.19.

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

`wit/app.wit` — копия контракта из `rustcloud/crates/plugin-api/wit/app.wit` (0.2.19).

## Установка

Администрирование → Плагины → установить `rc-talk.zip`, включить. Сообщения доставляет облако
(`jobs.add-for`); кто кому может писать — в системных настройках плагина: всем или только с общей
группой (по умолчанию).

Для звонков (позже) — адрес, ключ и секрет LiveKit в тех же настройках; адрес API LiveKit для
сервера — чтобы убирать из звонка исключённых из группы.
