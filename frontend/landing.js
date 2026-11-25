const buttons = document.querySelectorAll('[data-example]');
const examples = document.querySelectorAll('.example');

function selectExample(name) {
  for (const button of buttons) {
    button.setAttribute('aria-pressed', String(button.dataset.example === name));
  }
  for (const example of examples) {
    example.hidden = example.id !== name;
  }
}

for (const button of buttons) {
  button.addEventListener('click', () => selectExample(button.dataset.example));
}

for (const link of document.querySelectorAll('[data-select]')) {
  link.addEventListener('click', () => selectExample(link.dataset.select));
}

const root = document.documentElement;
const motionPreference = window.matchMedia('(prefers-reduced-motion: reduce)');
let frame = 0;

function updateScroll() {
  frame = 0;
  const available = root.scrollHeight - window.innerHeight;
  const progress = available > 0 ? window.scrollY / available : 0;
  root.style.setProperty('--scroll-progress', Math.min(1, Math.max(0, progress)));
}

function queueScrollUpdate() {
  if (!frame && !motionPreference.matches) frame = requestAnimationFrame(updateScroll);
}

window.addEventListener('scroll', queueScrollUpdate, { passive: true });
window.addEventListener('resize', queueScrollUpdate);
queueScrollUpdate();

if (window.gsap && window.ScrollTrigger) {
  gsap.registerPlugin(ScrollTrigger);
  gsap.matchMedia().add('(prefers-reduced-motion: no-preference)', () => {
    root.classList.add('cine');
    const cleanup = playFilm();
    joinHands();
    return () => {
      cleanup();
      root.classList.remove('cine');
    };
  });
} else {
  root.classList.remove('cine');
}

// Les deux mains se rejoignent pendant que la section entre à l’écran.
function joinHands() {
  gsap.fromTo('.trust-art',
    { xPercent: 14, yPercent: 16, rotation: 5, autoAlpha: 0 },
    {
      xPercent: 0, yPercent: 0, rotation: 0, autoAlpha: 1, ease: 'power2.out',
      scrollTrigger: { trigger: '.trust', start: 'top 85%', end: 'center 55%', scrub: .6 },
    });
}

// La scène se joue au défilement : chaque unité de la timeline vaut un dixième de la piste.
function playFilm() {
  const q = selector => document.querySelector(selector);
  const art = q('.film-art');
  const mascot = q('.film-mascot');
  const [step1, step2, step3] = document.querySelectorAll('[data-film-step]');
  const amount = q('.total-amount');
  const format = new Intl.NumberFormat('fr-FR');
  const counter = { value: 0 };
  const shown = { autoAlpha: 1, y: 0 };
  amount.textContent = '0';

  const tl = gsap.timeline({
    defaults: { ease: 'power2.inOut' },
    scrollTrigger: {
      trigger: '.film-track',
      start: 0,
      end: 'bottom bottom',
      // Le film rattrape le scroll en 1,2 s, puis se pose sur l’étape la plus proche dans le sens du défilement.
      scrub: 1.2,
      snap: { snapTo: 'labelsDirectional', delay: .2, duration: { min: .4, max: 1.2 }, ease: 'power1.inOut' },
      invalidateOnRefresh: true,
    },
  });

  tl.fromTo(mascot, {
    autoAlpha: 1,
    x: () => art.clientWidth / 2 - (mascot.offsetLeft + mascot.offsetWidth / 2),
    y: () => art.clientHeight / 2 - (mascot.offsetTop + mascot.offsetHeight / 2),
    scale: () => Math.min(art.clientWidth * .66, art.clientHeight * .7) / mascot.offsetWidth,
  }, { autoAlpha: 1, x: 0, y: 0, scale: 1, duration: 1.3 }, 0)
    .from('.orbit', { scale: 1.3, duration: 1.3 }, 0)
    .fromTo('.phone', { autoAlpha: 1, yPercent: 125 }, { autoAlpha: 1, yPercent: 0, duration: 1.3 }, .3)
    .to('.film-intro', { autoAlpha: 0, y: -40, duration: .6 }, .8)
    .fromTo(step1, { autoAlpha: 0, y: 40 }, { ...shown, duration: .6 }, 1.3)

    // 1. La photo : l’ordonnance entre dans l’écran, flash, puis lecture.
    .fromTo('.rx', { autoAlpha: 0, yPercent: -140, rotation: -14 }, { autoAlpha: 1, yPercent: 0, rotation: -3, duration: 1 }, 1.6)
    .to('.rx-flash', { opacity: .9, duration: .12, ease: 'none' }, 2.65)
    .to('.rx-flash', { opacity: 0, duration: .4, ease: 'power1.out' }, 2.77)
    .fromTo('.rx-scan', { top: '0%', opacity: 1 }, { top: '100%', opacity: 1, duration: 1.4, ease: 'none' }, 3)
    .to('.rx-scan', { opacity: 0, duration: .2 }, 4.4)

    // 2. Les lignes lues remplacent la photo.
    .to(step1, { autoAlpha: 0, y: -40, duration: .5 }, 4.5)
    .fromTo(step2, { autoAlpha: 0, y: 40 }, { ...shown, duration: .6 }, 4.9)
    .to('.rx', { autoAlpha: 0, scale: .85, yPercent: -20, duration: .6 }, 4.6)
    .fromTo('.row', { autoAlpha: 0, y: 30 }, { ...shown, duration: .5, stagger: .35 }, 5)
    .from('.row:not(.row-warning) span', { autoAlpha: 0, x: 12, duration: .4, stagger: .2 }, 7.2)

    // 3. Le sous-total et la garde.
    .to(step2, { autoAlpha: 0, y: -40, duration: .5 }, 6.7)
    .fromTo(step3, { autoAlpha: 0, y: 40 }, { ...shown, duration: .6 }, 7.1)
    .fromTo('.total', { autoAlpha: 0, y: 40 }, { ...shown, duration: .6 }, 7.6)
    .to(counter, {
      value: 6100, duration: 1.2, ease: 'power1.out',
      onUpdate: () => { amount.textContent = format.format(Math.round(counter.value / 100) * 100); },
    }, 7.9)
    .fromTo('.duty-chip', { autoAlpha: 0, y: -24, scale: .9 }, { autoAlpha: 1, y: 0, scale: 1, duration: .6, ease: 'back.out(1.6)' }, 9)
    .to({}, { duration: .6 })
    .addLabel('intro', 0)
    .addLabel('photo', 2.6)
    .addLabel('lecture', 6.3)
    .addLabel('budget', tl.duration());

  return () => { amount.textContent = '6 100'; };
}

if ('IntersectionObserver' in window) {
  const reveal = new IntersectionObserver(entries => {
    for (const entry of entries) {
      if (entry.isIntersecting) {
        entry.target.classList.add('is-visible');
        reveal.unobserve(entry.target);
      }
    }
  }, { threshold: .08 });
  for (const section of document.querySelectorAll('[data-reveal]')) reveal.observe(section);
  document.documentElement.classList.add('motion-ready');
}
