// Compte Hey-Doc : création de compte, connexion et déconnexion avec l’API (/v1/auth/*).
// La session est un cookie HttpOnly posé par l’API : la page ne voit jamais le jeton.
(() => {
  const section = document.querySelector('#inscription');
  if (!section) return;
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
  const forms = { register: section.querySelector('#register-form'), login: section.querySelector('#login-form') };
  const tabs = section.querySelector('.auth-tabs');
  const done = section.querySelector('.signup-done');
  const headerButton = document.querySelector('.header-account');

  const call = (path, options = {}) => fetch(`${api}${path}`, {
    credentials: 'include',
    ...options,
    headers: options.body ? { 'content-type': 'application/json' } : {},
  });

  function show(name) {
    for (const button of tabs.querySelectorAll('[data-auth-tab]')) {
      button.setAttribute('aria-pressed', String(button.dataset.authTab === name));
    }
    for (const [key, form] of Object.entries(forms)) form.hidden = key !== name;
  }

  for (const button of section.querySelectorAll('[data-auth-tab]')) {
    button.addEventListener('click', () => {
      show(button.dataset.authTab);
      forms[button.dataset.authTab].querySelector('input:not([type=hidden])').focus();
    });
  }

  for (const toggle of section.querySelectorAll('.show-password')) {
    toggle.addEventListener('click', () => {
      const input = toggle.previousElementSibling;
      const visible = input.type === 'password';
      input.type = visible ? 'text' : 'password';
      toggle.setAttribute('aria-pressed', String(visible));
      toggle.textContent = visible ? 'Masquer' : 'Afficher';
    });
  }

  // Un numéro saisi avec son indicatif (+…, 00…) est gardé tel quel. Sinon on ajoute celui du pays,
  // sans le 0 initial des numéros nationaux qui en ont un (France : 06 → +33 6). En Côte d’Ivoire,
  // le 0 fait partie des 10 chiffres.
  function whatsapp(form) {
    const dial = form.elements.country.selectedOptions[0].dataset.dial;
    const raw = form.elements.whatsapp.value.replace(/[\s.\-()]/g, '');
    if (raw.startsWith('+')) return raw;
    if (raw.startsWith('00')) return `+${raw.slice(2)}`;
    return `+${dial}${dial === '225' ? raw : raw.replace(/^0/, '')}`;
  }

  for (const form of Object.values(forms)) {
    form.elements.country.addEventListener('change', () => {
      form.querySelector('.dial').textContent = `+${form.elements.country.selectedOptions[0].dataset.dial}`;
    });
  }

  const messages = {
    first_name: 'Indiquez votre prénom.',
    last_name: 'Indiquez votre nom.',
    city: 'Indiquez votre ville.',
    whatsapp: 'Indiquez un numéro WhatsApp valide.',
    email: 'Cette adresse e-mail semble incomplète.',
    password: { register: 'Choisissez un mot de passe de 8 caractères au moins.', login: 'Indiquez votre mot de passe.' },
    consent: 'Votre accord est nécessaire pour créer un compte.',
  };

  function setError(form, name, message) {
    const input = form.elements[name];
    const error = name === 'consent' ? form.querySelector('.consent-error') : input.closest('.field').querySelector('.field-error');
    input.setAttribute('aria-invalid', message ? 'true' : 'false');
    error.textContent = message ?? '';
    error.hidden = !message;
    if (message) {
      error.id ||= `${form.id}-${name}-error`;
      input.setAttribute('aria-errormessage', error.id);
    }
  }

  function check(form) {
    const f = form.elements;
    const kind = form === forms.register ? 'register' : 'login';
    const invalid = { whatsapp: !/^\+[1-9]\d{7,14}$/.test(whatsapp(form)) };
    if (kind === 'register') {
      Object.assign(invalid, {
        first_name: !f.first_name.value.trim(),
        last_name: !f.last_name.value.trim(),
        city: !f.city.value.trim(),
        email: f.email.value.trim() !== '' && !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(f.email.value.trim()),
        password: f.password.value.length < 8,
        consent: !f.consent.checked,
      });
    } else {
      invalid.password = f.password.value === '';
    }
    for (const [name, bad] of Object.entries(invalid)) {
      const m = messages[name];
      setError(form, name, bad ? (typeof m === 'string' ? m : m[kind]) : null);
    }
    // Le premier champ en erreur dans l’ordre du formulaire.
    return [...f].find(el => el.getAttribute?.('aria-invalid') === 'true');
  }

  function payload(form) {
    const f = form.elements;
    if (form === forms.login) return { whatsapp: whatsapp(form), password: f.password.value };
    return {
      first_name: f.first_name.value.trim(),
      last_name: f.last_name.value.trim(),
      country: f.country.value,
      city: f.city.value.trim(),
      whatsapp: whatsapp(form),
      email: f.email.value.trim() || null,
      password: f.password.value,
      consent: true,
      website: f.website.value,
    };
  }

  const pretty = number => number.replace(/^(\+\d{3})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})$/, '$1 $2 $3 $4 $5 $6');

  function signedIn(account, focus) {
    for (const form of Object.values(forms)) {
      form.hidden = true;
      form.reset();
    }
    tabs.hidden = true;
    done.querySelector('.done-name').textContent = ` ${account.first_name}`;
    done.querySelector('.done-number').textContent = pretty(account.whatsapp);
    done.hidden = false;
    if (headerButton) {
      headerButton.textContent = 'Ouvrir le chat';
      headerButton.href = 'chat.html';
    }
    if (focus) done.focus();
  }

  function signedOut() {
    done.hidden = true;
    tabs.hidden = false;
    show('login');
    if (headerButton) {
      headerButton.textContent = 'Accéder au chat';
      headerButton.href = '#inscription';
    }
  }

  for (const form of Object.values(forms)) {
    const status = form.querySelector('.form-status');
    const submit = form.querySelector('.signup-submit');

    form.addEventListener('input', e => {
      if (e.target.getAttribute('aria-invalid') === 'true') check(form);
    });

    form.addEventListener('submit', async e => {
      e.preventDefault();
      status.textContent = '';
      status.classList.remove('info');
      const first = check(form);
      if (first) {
        first.focus();
        return;
      }
      const body = payload(form);
      submit.disabled = true;
      submit.setAttribute('aria-busy', 'true');
      submit.textContent = form === forms.login ? 'Connexion…' : 'Création du compte…';
      try {
        const res = await call(form.dataset.endpoint, { method: 'POST', body: JSON.stringify(body) });
        const data = await res.json().catch(() => ({}));
        if (res.ok) {
          // Connecté : direction l’espace de chat.
          location.href = 'chat.html';
          return;
        } else if (res.status === 409) {
          // Le numéro a déjà un compte : on bascule sur la connexion avec le numéro déjà rempli.
          show('login');
          forms.login.elements.country.value = forms.register.elements.country.value;
          forms.login.elements.country.dispatchEvent(new Event('change'));
          forms.login.elements.whatsapp.value = forms.register.elements.whatsapp.value;
          const notice = forms.login.querySelector('.form-status');
          notice.textContent = 'Un compte existe déjà pour ce numéro. Connectez-vous avec votre mot de passe.';
          notice.classList.add('info');
          forms.login.elements.password.focus();
        } else if (res.status === 401) {
          status.textContent = 'Numéro ou mot de passe incorrect.';
        } else if (res.status === 400 && data.message) {
          status.textContent = `Vérifiez le formulaire : ${data.message}`;
        } else {
          status.textContent = 'Le service n’a pas pu répondre. Réessayez dans un moment.';
        }
      } catch {
        status.textContent = 'Le service est injoignable. Vérifiez votre connexion et réessayez.';
      } finally {
        submit.disabled = false;
        submit.removeAttribute('aria-busy');
        submit.textContent = submit.dataset.label;
      }
    });
  }

  done.querySelector('.logout').addEventListener('click', async () => {
    try {
      await call('/v1/auth/logout', { method: 'POST' });
    } catch { /* La session expirera d’elle-même. */ }
    signedOut();
    forms.login.elements.whatsapp.focus();
  });

  // Une session déjà ouverte affiche directement le compte.
  // Session déjà ouverte : les accès au compte mènent au chat, et qui arrive sur la connexion
  // y est envoyé directement.
  call('/v1/auth/me')
    .then(res => (res.ok ? res.json() : null))
    .then(account => {
      if (!account) return;
      if (location.hash === '#inscription') {
        location.replace('chat.html');
        return;
      }
      signedIn(account, false);
      for (const link of document.querySelectorAll('a[href="#inscription"]')) link.href = 'chat.html';
    })
    .catch(() => {});
})();

// Lien WhatsApp avec « Hey Doc » déjà écrit, dès que le numéro du bot est renseigné.
(() => {
  const number = document.querySelector('meta[name="heydoc-whatsapp"]')?.content.replace(/\D/g, '');
  if (!number) return;
  for (const link of document.querySelectorAll('[data-whatsapp]')) {
    link.href = `https://wa.me/${number}?text=${encodeURIComponent('Hey Doc')}`;
    link.target = '_blank';
    link.rel = 'noopener';
  }
})();
