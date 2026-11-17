// The Talk page: one list of chats (requests to talk are chats too) and the
// open chat. It runs in the panel's sandboxed <iframe>; every request goes
// through rustcloud.fetch.
//
// New messages: the cloud delivers them into the user's AppData and tells
// the page with the event `changed` (rustcloud.onEvent); the page reloads.
(async () => {
  const TEXTS = {
    en: {
      search: 'Search', newContact: 'New contact', newGroup: 'New group', username: 'Username',
      contactHint: 'If this user may receive messages from you, they will see the request.',
      cancel: 'Cancel', ask: 'Send request', create: 'Create', groupTitle: 'Group name', members: 'Members',
      noContacts: 'Add contacts first: only they can be invited to a group.',
      accept: 'Accept', block: 'Block', unblock: 'Unblock', removeContact: 'Remove contact',
      wantsToTalk: 'wants to talk', wantsTitle: 'wants to add you to contacts',
      wantsText: 'Accept to start a chat. A blocked user can\'t send you anything.',
      requestSent: 'request sent', askedTitle: 'Request sent',
      askedText: 'When they accept it, the chat opens here.', cancelRequest: 'Cancel request',
      sayHello: 'Say hello!', groupCreated: 'Group created', blocked: 'You blocked this user.',
      notContact: 'Not in your contacts: you can\'t write here.',
      empty: 'Select a chat to start messaging', nothing: 'No chats yet. Tap the pencil to add a contact.',
      notFound: 'Nothing found', placeholder: 'Message',
      callsSoon: 'Calls will work once the cloud allows the camera and microphone for plugins.',
      tooMany: 'Too many messages at once: wait a minute.',
      edited: 'edited', deleted: 'Message deleted', older: 'Show older', left: 'You are no longer in this group.',
      edit: 'Edit', remove: 'Delete', failed: 'Something went wrong.', leave: 'Leave group',
      editingTitle: 'Editing', today: 'Today', yesterday: 'Yesterday', you: 'You',
      membersCount: { one: '# member', other: '# members' },
    },
    ru: {
      search: 'Поиск', newContact: 'Новый контакт', newGroup: 'Новая группа', username: 'Имя пользователя',
      contactHint: 'Если этому пользователю можно вам писать, он увидит запрос.',
      cancel: 'Отмена', ask: 'Отправить запрос', create: 'Создать', groupTitle: 'Название группы', members: 'Участники',
      noContacts: 'Сначала добавьте контакты: в группу можно пригласить только их.',
      accept: 'Принять', block: 'Заблокировать', unblock: 'Разблокировать', removeContact: 'Удалить контакт',
      wantsToTalk: 'хочет связаться с вами', wantsTitle: 'хочет добавить вас в контакты',
      wantsText: 'Примите, чтобы начать беседу. Заблокированный не сможет вам ничего отправить.',
      requestSent: 'запрос отправлен', askedTitle: 'Запрос отправлен',
      askedText: 'Когда его примут, здесь откроется беседа.', cancelRequest: 'Отменить запрос',
      sayHello: 'Напишите первым!', groupCreated: 'Группа создана', blocked: 'Вы заблокировали этого пользователя.',
      notContact: 'Не в ваших контактах: писать сюда нельзя.',
      empty: 'Выберите, кому хотели бы написать', nothing: 'Бесед пока нет. Нажмите на карандаш, чтобы добавить контакт.',
      notFound: 'Ничего не найдено', placeholder: 'Сообщение',
      callsSoon: 'Звонки заработают, когда облако разрешит плагинам камеру и микрофон.',
      tooMany: 'Слишком много сообщений сразу: подождите минуту.',
      edited: 'изменено', deleted: 'Сообщение удалено', older: 'Показать раньше', left: 'Вас больше нет в этой группе.',
      edit: 'Изменить', remove: 'Удалить', failed: 'Что-то пошло не так.', leave: 'Выйти из группы',
      editingTitle: 'Редактирование', today: 'Сегодня', yesterday: 'Вчера', you: 'Вы',
      membersCount: { one: '# участник', few: '# участника', many: '# участников', other: '# участника' },
    },
  };

  const context = await rustcloud.context;
  const locale = context.locale;
  const t = TEXTS[locale] || TEXTS.en;
  document.documentElement.dataset.theme = context.theme;
  document.documentElement.lang = locale;

  const $ = (id) => document.getElementById(id);
  const el = (tag, props = {}, ...children) => {
    const node = Object.assign(document.createElement(tag), props);
    node.append(...children.filter((c) => c !== null && c !== undefined && c !== false));
    return node;
  };
  const plural = (forms, n) => (forms[new Intl.PluralRules(locale).select(n)] || forms.other).replace('#', n);

  // "Alice (alice)" or "alice": the display name is never shown alone,
  // anyone may call themselves "Administrator".
  const label = (person) => {
    if (!person) return '?';
    const display = (person.display_name || '').trim();
    return !display || display === person.username ? person.username : `${display} (${person.username})`;
  };

  // ——— avatars: initials on a colour picked from the id ———
  function hue(key) {
    let h = 0;
    for (const ch of String(key)) h = (h * 31 + ch.codePointAt(0)) >>> 0;
    return h % 7;
  }
  function initials(text) {
    const words = String(text || '').replace(/\(.*\)/, '').split(/\s+/)
      .map((w) => w.replace(/[^\p{L}\p{N}]/gu, '')).filter(Boolean);
    const letters = words.length > 1 ? [words[0], words[1]] : [words[0] || '?'];
    return letters.map((w) => [...w][0]).join('').toUpperCase();
  }
  function avatar(key, text, size = '') {
    return el('span', { className: `avatar c${hue(key)} ${size}`, textContent: initials(text) });
  }
  function personName(person) {
    return (person?.display_name || '').trim() || person?.username || '?';
  }

  // ——— time ———
  const sameDay = (a, b) => a.toDateString() === b.toDateString();
  const clock = (d) => d.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
  function shortTime(iso) {
    if (!iso) return '';
    const d = new Date(iso);
    const now = new Date();
    if (sameDay(d, now)) return clock(d);
    if (now - d < 6 * 864e5) return d.toLocaleDateString(locale, { weekday: 'short' });
    if (d.getFullYear() === now.getFullYear()) return d.toLocaleDateString(locale, { day: 'numeric', month: 'short' });
    return d.toLocaleDateString(locale);
  }
  function dayTitle(d) {
    const now = new Date();
    if (sameDay(d, now)) return t.today;
    if (sameDay(d, new Date(now - 864e5))) return t.yesterday;
    const options = { day: 'numeric', month: 'long' };
    if (d.getFullYear() !== now.getFullYear()) options.year = 'numeric';
    return d.toLocaleDateString(locale, options);
  }

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
  let open = null;          // { kind: 'room' | 'incoming' | 'asked', id }
  let older = null;         // `before` of the next page
  let shown = [];           // messages of the open room
  let editing = null;       // the message being edited

  // ——— texts ———
  $('search').placeholder = t.search;
  $('empty').firstElementChild.textContent = t.empty;
  $('text').placeholder = t.placeholder;
  document.querySelectorAll('[data-t]').forEach((node) => { node.textContent = t[node.dataset.t]; });

  // ——— the list ———
  function entries() {
    const list = [];
    for (const [id, contact] of Object.entries(state.contacts)) {
      if (contact.state !== 'incoming') continue;
      list.push({ kind: 'incoming', id, key: id, title: label(contact), name: personName(contact),
        at: contact.since, preview: t.wantsToTalk, note: true, dot: true });
    }
    for (const [username, at] of Object.entries(state.asked)) {
      list.push({ kind: 'asked', id: username, key: username, title: username, name: username,
        at, preview: t.requestSent, note: true });
    }
    for (const room of Object.values(state.rooms)) {
      const peer = room.peer ? room.names[room.peer] : null;
      list.push({ kind: 'room', id: room.id, key: room.peer || room.id, title: room.title,
        name: peer ? personName(peer) : room.title, at: room.last_at, left: room.left,
        preview: room.last_text || (room.peer ? t.sayHello : t.groupCreated), note: !room.last_text,
        count: room.unread });
    }
    return list.sort((a, b) => (b.at || '').localeCompare(a.at || ''));
  }

  function renderSide() {
    const query = $('search').value.trim().toLowerCase();
    const all = entries();
    const list = query ? all.filter((e) => e.title.toLowerCase().includes(query)) : all;
    $('list').replaceChildren(...list.map((entry) => el('button', {
      className: 'item' + (open && open.kind === entry.kind && open.id === entry.id ? ' active' : '') +
        (entry.left ? ' left' : ''),
      onclick: () => show(entry.kind, entry.id),
    },
      avatar(entry.key, entry.name),
      el('span', { className: 'title', textContent: entry.title }),
      el('span', { className: 'time', textContent: shortTime(entry.at) }),
      el('span', { className: 'preview' + (entry.note ? ' note' : ''), textContent: entry.preview }),
      entry.count ? el('span', { className: 'count', textContent: String(entry.count) })
        : entry.dot ? el('span', { className: 'count dot' }) : el('span'),
    )));
    const emptyList = $('list-empty');
    emptyList.hidden = list.length > 0;
    emptyList.textContent = query ? t.notFound : t.nothing;
  }
  $('search').addEventListener('input', renderSide);

  // ——— the open chat ———
  function setHead(key, name, title, sub) {
    $('chat-avatar').replaceWith(Object.assign(avatar(key, name, 'small'), { id: 'chat-avatar' }));
    $('chat-title').textContent = title;
    $('chat-sub').textContent = sub;
  }

  function setBar(...buttons) {
    const bar = $('bar');
    bar.replaceChildren(...buttons);
    bar.hidden = buttons.length === 0;
  }
  const barButton = (text, onclick, danger) => el('button', { type: 'button', textContent: text, onclick, className: danger ? 'danger' : '' });
  const barNote = (text) => el('div', { className: 'note', textContent: text });

  async function show(kind, id) {
    closeMenu();
    stopEditing();
    open = { kind, id };
    document.body.classList.add('in-chat');
    $('empty').hidden = true;
    $('chat-head').hidden = false;
    $('messages').hidden = false;
    if (kind === 'room') {
      await showRoom(true);
    } else {
      showRequest();
    }
    renderSide();
  }

  function showRequest() {
    const { kind, id } = open;
    $('call').hidden = true;
    $('more').hidden = true;
    $('compose').hidden = true;
    shown = [];
    if (kind === 'incoming') {
      const contact = state.contacts[id];
      setHead(id, personName(contact), label(contact), contact.username);
      $('messages').replaceChildren(el('div', { className: 'card' },
        avatar(id, personName(contact), 'big'),
        el('h2', { textContent: label(contact) }),
        el('p', { textContent: t.wantsTitle }),
        el('p', { className: 'hint', textContent: t.wantsText }),
      ));
      setBar(
        barButton(t.block, () => act(() => api('/contacts/block', { id })), true),
        barButton(t.accept, () => act(() => api('/contacts/accept', { id }))),
      );
    } else {
      setHead(id, id, id, t.requestSent);
      $('messages').replaceChildren(el('div', { className: 'card' },
        avatar(id, id, 'big'),
        el('h2', { textContent: t.askedTitle }),
        el('p', { textContent: id }),
        el('p', { className: 'hint', textContent: t.askedText }),
      ));
      setBar(barButton(t.cancelRequest, () => act(() => api('/asked/cancel', { username: id })), true));
    }
  }

  async function showRoom(scroll) {
    const room = state.rooms[open.id];
    const peer = room.peer ? state.contacts[room.peer] : null;
    const peerName = room.peer ? room.names[room.peer] : null;
    setHead(room.peer || room.id, peerName ? personName(peerName) : room.title, room.title,
      room.peer ? (peerName?.username || '') : plural(t.membersCount, room.members.length));
    $('call').hidden = !state.calls || room.left;
    $('more').hidden = room.peer ? !peer : room.left;
    const writable = !room.left && (!room.peer || peer?.state === 'accepted');
    $('compose').hidden = !writable;
    if (writable) setBar();
    else if (peer?.state === 'blocked') setBar(barButton(t.unblock, () => act(() => api('/contacts/remove', { id: room.peer }))));
    else setBar(barNote(room.left ? t.left : t.notContact));
    await reloadRoom(scroll);
    // Seen as it came (not while the tab is hidden): no unread mark, no number in the side panel.
    if (room.unread && !document.hidden) {
      await api(`/rooms/${room.id}/read`, {});
      room.unread = 0;
    }
    if (writable && window.matchMedia('(pointer: fine)').matches) $('text').focus();
  }

  function renderMessages(scroll) {
    const box = $('messages');
    const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 80;
    const room = state.rooms[open.id];
    const items = [];
    if (older) items.push(el('button', { className: 'older', textContent: t.older, onclick: loadOlder }));
    let lastDay = '';
    shown.forEach((m, i) => {
      const at = new Date(m.at);
      const day = at.toDateString();
      if (day !== lastDay) items.push(el('div', { className: 'day', textContent: dayTitle(at) }));
      const prev = shown[i - 1];
      const next = shown[i + 1];
      const near = (a, b) => a && b && a.from === b.from && new Date(b.at) - new Date(a.at) < 5 * 60e3 &&
        new Date(a.at).toDateString() === new Date(b.at).toDateString();
      const first = !near(prev, m) || day !== lastDay;
      const last = !near(m, next);
      lastDay = day;
      const mine = m.from === state.me.id;
      const group = !room.peer && !mine;
      const person = room.names[m.from];
      const bubble = el('div', { className: 'msg' },
        group && first ? el('div', { className: `name n${hue(m.from)}`, textContent: label(person) }) : null,
        el('span', { className: 'text' + (m.deleted ? ' deleted' : ''), textContent: m.deleted ? t.deleted : m.text }),
        el('span', { className: 'meta', textContent: (m.edited && !m.deleted ? t.edited + ' ' : '') + clock(at) }),
      );
      if (mine && !m.deleted && !room.left) {
        bubble.addEventListener('contextmenu', (event) => {
          event.preventDefault();
          bubble.classList.add('menu-open');
          openMenu(event.clientX, event.clientY, [
            [t.edit, () => startEdit(m)],
            [t.remove, () => act(() => api(`/rooms/${open.id}/delete`, { msg: m.id, at: m.at })).then(() => reloadRoom()), true],
          ], () => bubble.classList.remove('menu-open'));
        });
      }
      items.push(el('div', { className: 'row' + (mine ? ' mine' : '') + (first ? '' : ' cont') + (last ? ' last' : '') },
        group ? (last ? avatar(m.from, personName(person), 'small') : el('span', { className: 'spacer' })) : null,
        bubble,
      ));
    });
    if (!shown.length && !older) {
      items.push(el('div', { className: 'service', textContent: room.peer ? t.sayHello : t.groupCreated }));
    }
    box.replaceChildren(...items);
    if (scroll || nearBottom) box.scrollTop = box.scrollHeight;
  }

  async function reloadRoom(scroll) {
    if (!open || open.kind !== 'room') return;
    const page = await api(`/rooms/${open.id}/messages`);
    shown = page.messages;
    older = page.older;
    renderMessages(scroll === true);
  }

  async function loadOlder() {
    const page = await api(`/rooms/${open.id}/messages?before=${encodeURIComponent(older)}`);
    shown = page.messages.concat(shown);
    older = page.older;
    renderMessages(false);
  }

  // ——— writing ———
  function startEdit(message) {
    editing = message;
    $('editing-text').textContent = message.text;
    $('editing').hidden = false;
    $('text').value = message.text;
    $('text').focus();
  }
  function stopEditing() {
    if (!editing) return;
    editing = null;
    $('editing').hidden = true;
    $('text').value = '';
  }
  $('editing-cancel').addEventListener('click', stopEditing);

  $('compose').addEventListener('submit', async (event) => {
    event.preventDefault();
    const text = $('text').value;
    if (!text.trim() || !open || open.kind !== 'room') return;
    const ok = await act(() => editing
      ? api(`/rooms/${open.id}/edit`, { msg: editing.id, at: editing.at, text })
      : api(`/rooms/${open.id}/messages`, { text }));
    if (ok) {
      $('text').value = '';
      stopEditing();
      await reloadRoom(true);
    }
  });
  $('text').addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      $('compose').requestSubmit();
    }
    if (event.key === 'Escape') stopEditing();
  });

  $('back').addEventListener('click', () => {
    document.body.classList.remove('in-chat');
    stopEditing();
    open = null;
    $('empty').hidden = false;
    for (const part of ['chat-head', 'messages', 'compose', 'bar']) $(part).hidden = true;
    renderSide();
  });

  $('call').addEventListener('click', () => {
    // The token works already; the call itself needs permissions.media from
    // the core (docs/design.md, "Calls").
    toast(t.callsSoon);
  });

  $('more').addEventListener('click', (event) => {
    const room = state.rooms[open.id];
    const box = event.currentTarget.getBoundingClientRect();
    const items = room.peer
      ? [
        [t.block, () => act(() => api('/contacts/block', { id: room.peer })), true],
        [t.removeContact, () => act(() => api('/contacts/remove', { id: room.peer })), true],
      ]
      : [[t.leave, () => act(() => api(`/rooms/${open.id}/leave`, {})), true]];
    if (room.peer && state.contacts[room.peer]?.state === 'blocked') items.shift();
    openMenu(box.right, box.bottom, items);
  });

  // ——— the menu ———
  let onMenuClose = null;
  function openMenu(x, y, items, onClose) {
    closeMenu();
    const menu = $('menu');
    menu.replaceChildren(...items.map(([text, action, danger]) => el('button', {
      type: 'button', textContent: text, className: danger ? 'danger' : '',
      onclick: () => { closeMenu(); action(); },
    })));
    menu.hidden = false;
    const { width, height } = menu.getBoundingClientRect();
    menu.style.left = Math.max(4, Math.min(x, innerWidth - width - 4)) + 'px';
    menu.style.top = Math.max(4, Math.min(y, innerHeight - height - 4)) + 'px';
    onMenuClose = onClose || null;
  }
  function closeMenu() {
    $('menu').hidden = true;
    if (onMenuClose) onMenuClose();
    onMenuClose = null;
  }
  document.addEventListener('pointerdown', (event) => { if (!$('menu').contains(event.target)) closeMenu(); });
  document.addEventListener('keydown', (event) => { if (event.key === 'Escape') closeMenu(); });
  $('messages').addEventListener('scroll', closeMenu);

  // ——— new chats ———
  $('new').addEventListener('click', (event) => {
    const box = event.currentTarget.getBoundingClientRect();
    openMenu(box.left - 140, box.top - 90, [
      [t.newContact, newContact],
      [t.newGroup, newGroup],
    ]);
  });

  function newContact() {
    const dialog = $('contact-dialog');
    dialog.querySelector('form').reset();
    dialog.showModal();
  }
  $('contact-dialog').addEventListener('close', async () => {
    const dialog = $('contact-dialog');
    if (dialog.returnValue !== 'ok') return;
    const username = dialog.querySelector('[name=username]').value.trim();
    if (username && await act(() => api('/contacts', { username }))) {
      if (state.asked[username]) show('asked', username);
    }
  });

  function newGroup() {
    const contacts = Object.entries(state.contacts).filter(([, c]) => c.state === 'accepted');
    if (!contacts.length) return toast(t.noContacts);
    const dialog = $('group-dialog');
    dialog.querySelector('form').reset();
    $('group-members').replaceChildren(...contacts.map(([id, c]) => el('label', {},
      avatar(id, personName(c), 'small'),
      el('span', { textContent: label(c) }),
      el('input', { type: 'checkbox', value: id }),
    )));
    dialog.showModal();
  }
  $('group-dialog').addEventListener('close', async () => {
    const dialog = $('group-dialog');
    if (dialog.returnValue !== 'ok') return;
    const title = dialog.querySelector('[name=title]').value;
    const members = [...dialog.querySelectorAll('#group-members input:checked')].map((i) => i.value);
    let created = null;
    if (await act(async () => { created = await api('/rooms', { title, members }); }) && created) {
      show('room', created.id);
    }
  });

  // alert() doesn't work in the sandbox: a toast instead.
  let toastTimer = null;
  function toast(text) {
    const box = $('toast');
    box.textContent = text;
    box.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { box.hidden = true; }, 4000);
  }

  async function act(action) {
    try {
      await action();
      await update();
      return true;
    } catch (error) {
      toast(error.code === 'TOO_MANY' ? t.tooMany : t.failed);
      return false;
    }
  }

  // ——— keeping up ———
  async function refresh() {
    state = await api('/state');
  }

  // What was open may have changed: a request accepted becomes the chat.
  function follow() {
    if (!open) return null;
    if (open.kind === 'room') return state.rooms[open.id] ? open : null;
    const peer = open.kind === 'incoming'
      ? (state.contacts[open.id]?.state === 'incoming' ? null : open.id)
      : (state.asked[open.id] ? null : Object.keys(state.contacts).find((id) => state.contacts[id].username === open.id && state.contacts[id].state === 'accepted'));
    if (!peer && (open.kind === 'incoming' ? state.contacts[open.id] : state.asked[open.id])) return open;
    const room = peer && Object.values(state.rooms).find((r) => r.peer === peer);
    return room ? { kind: 'room', id: room.id } : null;
  }

  // Something came or changed: reload what is shown. Events that come while
  // loading are folded into one more reload.
  let loading = null;
  let again = false;
  async function update() {
    if (loading) { again = true; return loading; }
    loading = (async () => {
      do {
        again = false;
        try {
          await refresh();
          const next = follow();
          if (!next) {
            if (open) $('back').click();
          } else if (next.kind !== open.kind || next.id !== open.id) {
            await show(next.kind, next.id);
          } else if (next.kind === 'room') {
            await showRoom(false);
          } else {
            showRequest();
          }
          renderSide();
        } catch { /* the next event or a return to the tab */ }
      } while (again);
    })();
    await loading;
    loading = null;
  }

  rustcloud.onEvent((event) => { if (event.type === 'changed') update(); });
  // The panel's events reach only an open page: catch up on return.
  document.addEventListener('visibilitychange', () => { if (!document.hidden) update(); });

  await refresh();
  renderSide();
})();
