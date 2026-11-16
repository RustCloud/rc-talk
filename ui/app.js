// The Talk page: contacts, chats and messages. It runs in the panel's
// sandboxed <iframe>; every request goes through rustcloud.fetch.
//
// New messages: until the cloud has the SSE event `changed`
// (rustcloud.onEvent), the page asks /api/sync every few seconds while it is
// visible and rarely while hidden.
(async () => {
  const TEXTS = {
    en: {
      addContact: '+ Contact', addGroup: '+ Group', username: 'Username',
      contactHint: 'They will see your request when they open Talk.',
      cancel: 'Cancel', ask: 'Send request', create: 'Create', groupTitle: 'Name', members: 'Members',
      requests: 'Requests', accept: 'Accept', block: 'Block', waiting: 'waiting for answer',
      empty: 'Choose a chat or add a contact.', send: 'Send', placeholder: 'Message',
      call: 'Call', callsSoon: 'Calls will work once the cloud allows the camera for plugins.',
      deliveryOff: 'Delivery is off on this server: ask the administrator.',
      edited: 'edited', deleted: 'Message deleted', older: 'Older messages', left: 'You are no longer in this group.',
      edit: 'Edit', remove: 'Delete', failed: 'Something went wrong.', leave: 'Leave',
    },
    ru: {
      addContact: '+ Контакт', addGroup: '+ Группа', username: 'Имя пользователя',
      contactHint: 'Человек увидит запрос, когда откроет «Общение».',
      cancel: 'Отмена', ask: 'Отправить запрос', create: 'Создать', groupTitle: 'Название', members: 'Участники',
      requests: 'Запросы', accept: 'Принять', block: 'Заблокировать', waiting: 'ждёт ответа',
      empty: 'Выберите беседу или добавьте контакт.', send: 'Отправить', placeholder: 'Сообщение',
      call: 'Позвонить', callsSoon: 'Звонки заработают, когда облако разрешит плагинам камеру.',
      deliveryOff: 'Доставка на этом сервере выключена: обратитесь к администратору.',
      edited: 'изменено', deleted: 'Сообщение удалено', older: 'Раньше', left: 'Вас больше нет в этой группе.',
      edit: 'Изменить', remove: 'Удалить', failed: 'Что-то пошло не так.', leave: 'Выйти',
    },
  };

  const context = await rustcloud.context;
  const t = TEXTS[context.locale] || TEXTS.en;
  document.documentElement.dataset.theme = context.theme;
  document.documentElement.lang = context.locale;

  const $ = (id) => document.getElementById(id);
  // "Alice (alice)" or "alice": the display name is never shown alone,
  // anyone may call themselves "Administrator".
  const label = (person) => {
    if (!person) return '?';
    const display = (person.display_name || '').trim();
    return !display || display === person.username ? person.username : `${display} (${person.username})`;
  };
  const el = (tag, props = {}, ...children) => {
    const node = Object.assign(document.createElement(tag), props);
    node.append(...children);
    return node;
  };

  async function api(path, body) {
    const options = body === undefined ? {} : {
      method: 'POST',
      body: JSON.stringify(body),
      headers: { 'content-type': 'application/json' },
    };
    const res = await rustcloud.fetch('/api' + path, options);
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw Object.assign(new Error(data.error || res.status), { code: data.error });
    return data;
  }

  let state = null;
  let current = null;       // room id
  let older = null;         // `before` of the next page
  let shown = [];           // messages of the current room

  // ——— texts ———
  $('add-contact').textContent = t.addContact;
  $('add-group').textContent = t.addGroup;
  $('empty').textContent = t.empty;
  $('send').textContent = t.send;
  $('call').textContent = t.call;
  $('text').placeholder = t.placeholder;
  document.querySelectorAll('[data-t]').forEach((node) => { node.textContent = t[node.dataset.t]; });

  // ——— the list ———
  function renderSide() {
    const notice = $('notice');
    notice.hidden = state.delivery !== 'off';
    notice.textContent = t.deliveryOff;

    const requests = $('requests');
    requests.replaceChildren();
    const incoming = Object.entries(state.contacts).filter(([, c]) => c.state === 'incoming');
    const asked = Object.keys(state.asked);
    if (incoming.length || asked.length) requests.append(el('h2', { textContent: t.requests }));
    for (const [id, contact] of incoming) {
      requests.append(el('div', { className: 'request' },
        el('span', { textContent: label(contact) }),
        el('button', { textContent: t.accept, onclick: () => act(() => api('/contacts/accept', { id })) }),
        el('button', { textContent: t.block, className: 'quiet', onclick: () => act(() => api('/contacts/block', { id })) }),
      ));
    }
    for (const username of asked) {
      requests.append(el('div', { className: 'request muted' },
        el('span', { textContent: `${username} — ${t.waiting}` }),
        el('button', { textContent: '×', className: 'quiet', onclick: () => act(() => api('/asked/cancel', { username })) }),
      ));
    }

    const rooms = Object.values(state.rooms).sort((a, b) => (b.last_at || '').localeCompare(a.last_at || ''));
    $('rooms').replaceChildren(...rooms.map((room) => el('button', {
      className: 'room' + (room.id === current ? ' active' : '') + (room.left ? ' muted' : ''),
      onclick: () => openRoom(room.id),
    },
      el('span', { className: 'title', textContent: room.title }),
      room.unread ? el('span', { className: 'badge', textContent: String(room.unread) }) : '',
      el('span', { className: 'last', textContent: room.last_text }),
    )));
  }

  // ——— a chat ———
  function renderMessages(scroll) {
    const box = $('messages');
    const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 80;
    const items = [];
    if (older) items.push(el('button', { className: 'older', textContent: t.older, onclick: loadOlder }));
    const room = state.rooms[current];
    if (room?.left) items.push(el('p', { className: 'muted', textContent: t.left }));
    let lastDay = '';
    for (const m of shown) {
      const at = new Date(m.at);
      const day = at.toLocaleDateString(context.locale);
      if (day !== lastDay) items.push(el('div', { className: 'day', textContent: (lastDay = day) }));
      const mine = m.from === state.me.id;
      const meta = el('span', { className: 'meta', textContent:
        (room?.peer || mine ? '' : label(room?.names[m.from]) + ' · ') +
        at.toLocaleTimeString(context.locale, { hour: '2-digit', minute: '2-digit' }) +
        (m.edited ? ' · ' + t.edited : '') });
      const text = el('div', { className: 'text' + (m.deleted ? ' muted' : ''), textContent: m.deleted ? t.deleted : m.text });
      const bubble = el('div', { className: 'msg' + (mine ? ' mine' : '') }, text, meta);
      if (mine && !m.deleted) {
        bubble.append(el('span', { className: 'actions' },
          el('button', { textContent: t.edit, className: 'quiet', onclick: () => startEdit(m) }),
          el('button', { textContent: t.remove, className: 'quiet', onclick: () => act(() => api(`/rooms/${current}/delete`, { msg: m.id, at: m.at })).then(reloadRoom) }),
        ));
      }
      items.push(bubble);
    }
    box.replaceChildren(...items);
    if (scroll || nearBottom) box.scrollTop = box.scrollHeight;
  }

  async function openRoom(id) {
    current = id;
    document.body.classList.add('in-chat');
    $('empty').hidden = true;
    for (const part of ['chat-head', 'messages', 'compose']) $(part).hidden = false;
    const room = state.rooms[id];
    $('chat-title').textContent = room.title;
    $('call').hidden = !state.calls;
    $('leave').hidden = !!room.peer || room.left;
    $('compose').hidden = room.left;
    await reloadRoom(true);
    if (room.unread) {
      await api(`/rooms/${id}/read`, {});
      room.unread = 0;
    }
    renderSide();
    $('text').focus();
  }

  async function reloadRoom(scroll) {
    if (!current) return;
    const page = await api(`/rooms/${current}/messages`);
    shown = page.messages;
    older = page.older;
    renderMessages(scroll === true);
  }

  async function loadOlder() {
    const page = await api(`/rooms/${current}/messages?before=${encodeURIComponent(older)}`);
    shown = page.messages.concat(shown);
    older = page.older;
    renderMessages(false);
  }

  let editing = null;
  function startEdit(message) {
    editing = message;
    $('text').value = message.text;
    $('text').focus();
  }

  $('compose').addEventListener('submit', async (event) => {
    event.preventDefault();
    const text = $('text').value;
    if (!text.trim() || !current) return;
    const ok = await act(() => editing
      ? api(`/rooms/${current}/edit`, { msg: editing.id, at: editing.at, text })
      : api(`/rooms/${current}/messages`, { text }));
    if (ok) {
      $('text').value = '';
      editing = null;
      await reloadRoom(true);
      await refresh();
    }
  });
  $('text').addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      $('compose').requestSubmit();
    }
    if (event.key === 'Escape' && editing) {
      editing = null;
      $('text').value = '';
    }
  });

  $('back').addEventListener('click', () => {
    document.body.classList.remove('in-chat');
    current = null;
    renderSide();
  });

  $('leave').textContent = t.leave;
  $('leave').addEventListener('click', async () => {
    if (await act(() => api(`/rooms/${current}/leave`, {}))) openRoom(current);
  });

  $('call').addEventListener('click', async () => {
    // The token works already; the call itself needs permissions.connect and
    // permissions.media from the core (docs/design.md, "Calls").
    alertBox(t.callsSoon);
  });

  // ——— dialogs ———
  $('add-contact').addEventListener('click', () => {
    const dialog = $('contact-dialog');
    dialog.querySelector('form').reset();
    dialog.showModal();
  });
  $('contact-dialog').addEventListener('close', async () => {
    const dialog = $('contact-dialog');
    if (dialog.returnValue !== 'ok') return;
    const username = dialog.querySelector('[name=username]').value.trim();
    if (username) await act(() => api('/contacts', { username }));
  });

  $('add-group').addEventListener('click', () => {
    const dialog = $('group-dialog');
    dialog.querySelector('form').reset();
    const box = $('group-members');
    box.querySelectorAll('label').forEach((node) => node.remove());
    for (const [id, c] of Object.entries(state.contacts)) {
      if (c.state !== 'accepted') continue;
      box.append(el('label', {}, el('input', { type: 'checkbox', value: id }), ' ' + label(c)));
    }
    dialog.showModal();
  });
  $('group-dialog').addEventListener('close', async () => {
    const dialog = $('group-dialog');
    if (dialog.returnValue !== 'ok') return;
    const title = dialog.querySelector('[name=title]').value;
    const members = [...dialog.querySelectorAll('#group-members input:checked')].map((i) => i.value);
    await act(() => api('/rooms', { title, members }));
  });

  // alert() doesn't work in the sandbox: a small banner instead.
  function alertBox(text) {
    const notice = $('notice');
    notice.textContent = text;
    notice.hidden = false;
    setTimeout(() => renderSide(), 5000);
  }

  async function act(action) {
    try {
      await action();
      await refresh();
      return true;
    } catch (error) {
      alertBox(error.code === 'DELIVERY_OFF' ? t.deliveryOff : t.failed);
      return false;
    }
  }

  // ——— keeping up ———
  async function refresh() {
    state = await api('/state');
    renderSide();
  }

  async function sync() {
    try {
      const { taken } = await api('/sync', {});
      if (taken) {
        await refresh();
        if (current) await reloadRoom();
      }
    } catch { /* next time */ }
  }

  let timer = null;
  function schedule() {
    clearTimeout(timer);
    timer = setTimeout(async () => { await sync(); schedule(); }, document.hidden ? 30000 : 3000);
  }
  document.addEventListener('visibilitychange', () => { if (!document.hidden) sync(); schedule(); });
  if (rustcloud.onEvent) rustcloud.onEvent(() => sync());

  await refresh();
  await sync();
  schedule();
})();
