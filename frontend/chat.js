// Chat Hey-Doc. La session est le cookie HttpOnly posé par l’API à la connexion : sans session,
// la page renvoie vers la connexion. La conversation ouverte est dans l’adresse (#c=<id>).
(() => {
  // L’API doit être appelée sur le même nom d’hôte que la page (localhost ou 127.0.0.1) :
  // sinon le navigateur n’envoie pas le cookie de session.
  const api = (() => {
    const meta = document.querySelector('meta[name="heydoc-api"]')?.content;
    if (!meta) return '';
    const url = new URL(meta, location.href);
    const loopback = ['localhost', '127.0.0.1', '[::1]'];
    if (loopback.includes(url.hostname) && loopback.includes(location.hostname)) url.hostname = location.hostname;
    return url.origin;
  })();
  const $ = selector => document.querySelector(selector);
  const app = $('.app');
  const thread = $('.thread');
  const list = $('.messages');
  const historyList = $('.history-list');
  const form = $('.composer');
  const input = $('#message');
  const send = $('.send');
  const file = $('.file');
  const title = $('.chat-title');

  const state = { account: null, conversations: [], current: null, busy: false };
  // Aperçus des photos envoyées pendant cette visite : la photo n’est pas conservée par Hey-Doc.
  const previews = new Map();

  const call = async (path, options = {}) => {
    const res = await fetch(`${api}${path}`, { credentials: 'include', ...options });
    if (res.status === 401) {
      location.replace('index.html#inscription');
      throw new Error('session');
    }
    return res;
  };

  // --- Texte des réponses : paragraphes, listes et gras, sans HTML venant du modèle ---------

  const escape = s => s.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const inline = s => escape(s).replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');

  function rich(text) {
    const out = [];
    let para = [];
    let items = null;
    const flushPara = () => { if (para.length) out.push(`<p>${para.map(inline).join('<br>')}</p>`); para = []; };
    const flushList = () => { if (items) out.push(`<${items.tag}>${items.lines.map(l => `<li>${inline(l)}</li>`).join('')}</${items.tag}>`); items = null; };
    for (const line of text.split('\n')) {
      const bullet = line.match(/^\s*[-*•]\s+(.*)$/);
      const number = line.match(/^\s*\d+[.)]\s+(.*)$/);
      if (bullet || number) {
        flushPara();
        const tag = bullet ? 'ul' : 'ol';
        if (items?.tag !== tag) { flushList(); items = { tag, lines: [] }; }
        items.lines.push((bullet ?? number)[1]);
      } else if (!line.trim()) {
        flushPara();
        flushList();
      } else {
        flushList();
        para.push(line);
      }
    }
    flushPara();
    flushList();
    return out.join('');
  }

  // --- Messages ---------------------------------------------------------------------------

  const statusLabel = { matched: 'Trouvé', ambiguous: 'À choisir', no_match: 'À confirmer' };

  function prescriptionCard(data) {
    if (!data || data.status === 'unreadable' || !data.lines?.length) return '';
    const lines = data.lines.map(line => {
      const meta = [
        line.quantity ? `${line.quantity} boîte${line.quantity > 1 ? 's' : ''}` : null,
        line.units ? `${line.units} unités` : null,
        line.instructions?.length ? line.instructions.join(' · ') : null,
      ].filter(Boolean).join(' · ');
      const candidates = (line.candidates ?? []).slice(0, 3).map(c =>
        `<li><span>${escape(c.label)}</span><small>${escape([c.dosage_form, c.pack].filter(Boolean).join(' · '))}</small></li>`).join('');
      return `<div class="rx-line">
        <div class="rx-line-head"><strong>${escape(line.text)}</strong><span class="badge ${line.status}">${statusLabel[line.status] ?? ''}</span></div>
        ${meta ? `<p class="rx-meta">${escape(meta)}</p>` : ''}
        ${candidates ? `<ul class="candidates">${candidates}</ul>` : ''}
      </div>`;
    }).join('');
    const n = data.lines.length;
    return `<div class="rx-card">
      <header><strong>Ordonnance relevée</strong><span>${n} ligne${n > 1 ? 's' : ''}</span></header>
      ${lines}
      <p class="rx-foot">Montrez ces lignes à votre pharmacien : il confirme le produit, le dosage et la boîte.</p>
    </div>`;
  }

  function messageElement(m) {
    const li = document.createElement('li');
    li.className = `msg ${m.role}`;
    if (m.role === 'user') {
      if (m.kind === 'photo') {
        const src = previews.get(m.id) ?? m.preview;
        li.innerHTML = `<div class="photo">${src ? `<img src="${src}" alt="Photo d’ordonnance envoyée">` : ''}<span class="photo-chip">📷 Photo d’ordonnance</span></div>`;
      } else {
        li.innerHTML = '<div class="bubble"></div>';
        li.querySelector('.bubble').textContent = m.content;
      }
      return li;
    }
    const body = m.kind === 'prescription'
      ? `${rich(m.data?.lines?.length ? m.content.split('\n')[0] : m.content)}${prescriptionCard(m.data)}`
      : rich(m.content);
    li.innerHTML = `<img class="face" src="../branding/heydoc-mascotte.svg" width="32" height="32" alt=""><div class="body">${body}</div>`;
    return li;
  }

  function pendingElement(text) {
    const li = document.createElement('li');
    li.className = 'msg assistant pending';
    li.innerHTML = `<img class="face" src="../branding/heydoc-mascotte.svg" width="32" height="32" alt=""><div class="body"><span class="typing" aria-hidden="true"><i></i><i></i><i></i></span><p class="status-line">${escape(text)}</p></div>`;
    return li;
  }

  const scrollDown = () => requestAnimationFrame(() => { thread.scrollTop = thread.scrollHeight; });

  function showMessages(messages) {
    list.replaceChildren(...messages.map(messageElement));
    thread.classList.toggle('has-messages', messages.length > 0);
    scrollDown();
  }

  // --- Conversations ----------------------------------------------------------------------

  function renderHistory() {
    const items = state.conversations.map(c => {
      const li = document.createElement('li');
      li.className = 'history-item';
      if (c.id === state.current) li.setAttribute('aria-current', 'true');
      li.innerHTML = `<a href="#c=${c.id}"></a><button type="button" class="icon-button delete" aria-label="Supprimer la conversation"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M5 7h14M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/></svg></button>`;
      li.querySelector('a').textContent = c.title;
      li.querySelector('.delete').addEventListener('click', () => remove(c));
      return li;
    });
    historyList.replaceChildren(...items);
    $('.history-empty').hidden = items.length > 0;
  }

  async function loadHistory() {
    const res = await call('/v1/chat/conversations');
    state.conversations = res.ok ? await res.json() : [];
    renderHistory();
  }

  function newConversation() {
    state.current = null;
    if (location.hash) window.history.replaceState(null, '', location.pathname);
    title.textContent = 'Nouvelle conversation';
    document.title = 'Chat · Hey-Doc';
    showMessages([]);
    renderHistory();
    closeMenu();
    input.focus();
  }

  async function open(id) {
    if (state.busy) return;
    const res = await call(`/v1/chat/conversations/${id}`);
    if (!res.ok) return newConversation();
    const c = await res.json();
    state.current = c.id;
    title.textContent = c.title;
    document.title = `${c.title} · Hey-Doc`;
    showMessages(c.messages);
    renderHistory();
    closeMenu();
  }

  async function remove(c) {
    if (!confirm(`Supprimer « ${c.title} » ? Cette conversation sera effacée définitivement.`)) return;
    const res = await call(`/v1/chat/conversations/${c.id}`, { method: 'DELETE' });
    if (!res.ok && res.status !== 404) return;
    state.conversations = state.conversations.filter(x => x.id !== c.id);
    if (state.current === c.id) newConversation();
    else renderHistory();
  }

  function routeFromHash() {
    const id = location.hash.match(/^#c=([0-9a-f-]{36})$/)?.[1];
    if (id && id !== state.current) open(id);
    else if (!id && state.current) newConversation();
  }

  // --- Envoi ------------------------------------------------------------------------------

  function setBusy(busy) {
    state.busy = busy;
    send.disabled = busy || !input.value.trim();
    $('.attach').disabled = busy;
  }

  async function exchange(request, userMessage, waiting) {
    setBusy(true);
    thread.classList.add('has-messages');
    const mine = messageElement(userMessage);
    const pending = pendingElement(waiting);
    list.append(mine, pending);
    scrollDown();
    try {
      const res = await call(request.path, request.options);
      const data = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(data.message || 'Hey-Doc n’a pas pu répondre. Réessayez dans un moment.');
      const [user, reply] = data.messages;
      if (userMessage.preview) previews.set(user.id, userMessage.preview);
      mine.replaceWith(messageElement(user));
      pending.replaceWith(messageElement(reply));
      const conv = data.conversation;
      state.current = conv.id;
      state.conversations = [conv, ...state.conversations.filter(c => c.id !== conv.id)];
      title.textContent = conv.title;
      document.title = `${conv.title} · Hey-Doc`;
      window.history.replaceState(null, '', `#c=${conv.id}`);
      renderHistory();
    } catch (err) {
      if (err.message === 'session') return;
      const message = err instanceof TypeError ? 'Le service est injoignable. Vérifiez votre connexion.' : err.message;
      pending.classList.remove('pending');
      pending.querySelector('.body').innerHTML = `<div class="error" role="alert"><span>${escape(message)}</span><button type="button">Réessayer</button></div>`;
      pending.querySelector('.error button').addEventListener('click', () => {
        mine.remove();
        pending.remove();
        exchange(request, userMessage, waiting);
      });
    } finally {
      setBusy(false);
      scrollDown();
    }
  }

  function sendText(text) {
    const body = JSON.stringify({ conversation_id: state.current, text });
    exchange(
      { path: '/v1/chat/messages', options: { method: 'POST', headers: { 'content-type': 'application/json' }, body } },
      { role: 'user', kind: 'text', content: text },
      'Hey-Doc réfléchit…',
    );
  }

  function sendPhoto(photo) {
    const allowed = ['image/jpeg', 'image/png', 'image/heic', 'image/heif'];
    if (photo.type && !allowed.includes(photo.type)) return alert('Envoyez une photo JPEG, PNG ou HEIC.');
    if (photo.size > 10 * 1024 * 1024) return alert('La photo dépasse 10 Mo. Reprenez-la ou réduisez sa taille.');
    const data = new FormData();
    data.append('image', photo);
    if (state.current) data.append('conversation_id', state.current);
    const preview = photo.type.startsWith('image/h') ? null : URL.createObjectURL(photo);
    exchange(
      { path: '/v1/chat/photo', options: { method: 'POST', body: data } },
      { role: 'user', kind: 'photo', content: 'Photo d’ordonnance', preview },
      'Lecture de l’ordonnance…',
    );
  }

  function resize() {
    input.style.height = 'auto';
    input.style.height = `${Math.min(input.scrollHeight, 220)}px`;
    send.disabled = state.busy || !input.value.trim();
  }

  form.addEventListener('submit', e => {
    e.preventDefault();
    const text = input.value.trim();
    if (!text || state.busy) return;
    input.value = '';
    resize();
    sendText(text);
  });

  input.addEventListener('input', resize);
  input.addEventListener('keydown', e => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      form.requestSubmit();
    }
  });

  $('.attach').addEventListener('click', () => file.click());
  file.addEventListener('change', () => {
    if (file.files[0] && !state.busy) sendPhoto(file.files[0]);
    file.value = '';
  });

  for (const button of document.querySelectorAll('.suggestions button')) {
    button.addEventListener('click', () => {
      if (button.dataset.action === 'photo') return file.click();
      input.value = button.dataset.prompt;
      resize();
      input.focus();
    });
  }

  // --- Menu et compte ---------------------------------------------------------------------

  const menuButton = $('.open-sidebar');
  function closeMenu() {
    app.classList.remove('menu-open');
    menuButton.setAttribute('aria-expanded', 'false');
    $('.scrim').hidden = true;
  }
  menuButton.addEventListener('click', () => {
    app.classList.add('menu-open');
    menuButton.setAttribute('aria-expanded', 'true');
    $('.scrim').hidden = false;
    $('.new-chat').focus();
  });
  $('.close-sidebar').addEventListener('click', closeMenu);
  $('.scrim').addEventListener('click', closeMenu);
  document.addEventListener('keydown', e => { if (e.key === 'Escape' && app.classList.contains('menu-open')) { closeMenu(); menuButton.focus(); } });
  $('.new-chat').addEventListener('click', newConversation);
  $('.new-chat-top').addEventListener('click', newConversation);
  window.addEventListener('hashchange', routeFromHash);

  $('.logout').addEventListener('click', async () => {
    try { await call('/v1/auth/logout', { method: 'POST' }); } catch { /* la session expirera */ }
    location.href = 'index.html';
  });

  (async () => {
    try {
      const res = await call('/v1/auth/me');
      if (!res.ok) throw new Error('indisponible');
      const account = await res.json();
      state.account = account;
      $('.welcome-name').textContent = ` ${account.first_name}`;
      $('.avatar').textContent = account.first_name.charAt(0).toUpperCase();
      $('.account-name').textContent = `${account.first_name} ${account.last_name}`;
      $('.account-number').textContent = account.whatsapp;
      await loadHistory();
      app.dataset.state = 'ready';
      routeFromHash();
      input.focus();
    } catch (err) {
      if (err.message === 'session') return;
      app.dataset.state = 'ready';
      list.innerHTML = '<li class="msg assistant"><div class="body"><div class="error" role="alert">Le service Hey-Doc est injoignable. Réessayez dans un moment.</div></div></li>';
      thread.classList.add('has-messages');
    }
  })();
})();
