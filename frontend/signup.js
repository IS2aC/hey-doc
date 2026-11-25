// Inscription au chat : vérifie le formulaire, puis l’envoie à l’API Hey-Doc (POST /v1/signups).
(() => {
  const form = document.querySelector('.signup-form');
  if (!form) return;
  const api = document.querySelector('meta[name="heydoc-api"]')?.content ?? '';
  const done = document.querySelector('.signup-done');
  const status = form.querySelector('.form-status');
  const submit = form.querySelector('.signup-submit');
  const country = form.elements.country;
  const dial = form.querySelector('.dial');

  const dialCode = () => country.selectedOptions[0].dataset.dial;
  country.addEventListener('change', () => { dial.textContent = `+${dialCode()}`; });

  // Un numéro saisi avec son indicatif (+…, 00…) est gardé tel quel. Sinon on ajoute celui du pays,
  // sans le 0 initial des numéros nationaux qui en ont un (France : 06 → +33 6).
  function whatsapp() {
    const raw = form.elements.whatsapp.value.replace(/[\s.\-()]/g, '');
    if (raw.startsWith('+')) return raw;
    if (raw.startsWith('00')) return `+${raw.slice(2)}`;
    const national = dialCode() === '225' ? raw : raw.replace(/^0/, '');
    return `+${dialCode()}${national}`;
  }

  const messages = {
    first_name: 'Indiquez votre prénom.',
    last_name: 'Indiquez votre nom.',
    city: 'Indiquez votre ville.',
    whatsapp: 'Indiquez un numéro WhatsApp valide.',
    email: 'Cette adresse e-mail semble incomplète.',
    consent: 'Votre accord est nécessaire pour vous inscrire.',
  };

  function setError(name, message) {
    const input = form.elements[name];
    const error = name === 'consent' ? form.querySelector('.consent-error') : input.closest('.field').querySelector('.field-error');
    input.setAttribute('aria-invalid', message ? 'true' : 'false');
    error.textContent = message ?? '';
    error.hidden = !message;
    if (message) {
      error.id ||= `${name}-error`;
      input.setAttribute('aria-errormessage', error.id);
    }
  }

  function check() {
    const f = form.elements;
    const invalid = {
      first_name: !f.first_name.value.trim(),
      last_name: !f.last_name.value.trim(),
      city: !f.city.value.trim(),
      whatsapp: !/^\+[1-9]\d{7,14}$/.test(whatsapp()),
      email: f.email.value.trim() !== '' && !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(f.email.value.trim()),
      consent: !f.consent.checked,
    };
    for (const [name, bad] of Object.entries(invalid)) setError(name, bad ? messages[name] : null);
    return Object.keys(invalid).find(name => invalid[name]);
  }

  // Une erreur corrigée disparaît dès la saisie suivante.
  form.addEventListener('input', e => {
    if (e.target.getAttribute('aria-invalid') === 'true') check();
  });

  form.addEventListener('submit', async e => {
    e.preventDefault();
    status.textContent = '';
    const first = check();
    if (first) {
      form.elements[first].focus();
      return;
    }
    const f = form.elements;
    const body = {
      first_name: f.first_name.value.trim(),
      last_name: f.last_name.value.trim(),
      country: f.country.value,
      city: f.city.value.trim(),
      whatsapp: whatsapp(),
      email: f.email.value.trim() || null,
      consent: true,
      website: f.website.value,
    };
    submit.setAttribute('aria-busy', 'true');
    submit.disabled = true;
    submit.textContent = 'Inscription…';
    try {
      const res = await fetch(`${api}/v1/signups`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(body),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        throw new Error(res.status === 400 && err.message ? `Vérifiez le formulaire : ${err.message}` : 'L’inscription n’a pas abouti. Réessayez dans un moment.');
      }
      form.hidden = true;
      done.querySelector('.done-name').textContent = `, ${body.first_name}`;
      done.querySelector('.done-number').textContent = body.whatsapp.replace(/^(\+\d{3})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})$/, '$1 $2 $3 $4 $5 $6');
      done.hidden = false;
      done.focus();
    } catch (err) {
      status.textContent = err instanceof TypeError ? 'Le service est injoignable. Vérifiez votre connexion et réessayez.' : err.message;
    } finally {
      submit.removeAttribute('aria-busy');
      submit.disabled = false;
      submit.textContent = 'Accéder au chat';
    }
  });
})();
