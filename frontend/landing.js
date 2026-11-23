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

const storySteps = [...document.querySelectorAll('[data-step]')];
const stage = document.querySelector('.story-stage');
const hero = document.querySelector('.hero-art');
const motionPreference = window.matchMedia('(prefers-reduced-motion: reduce)');
let frame = 0;

function updateScroll() {
  frame = 0;
  const root = document.documentElement;
  const available = root.scrollHeight - window.innerHeight;
  const progress = available > 0 ? window.scrollY / available : 0;
  root.style.setProperty('--scroll-progress', Math.min(1, Math.max(0, progress)));
  hero.style.setProperty('--hero-drift', `${Math.min(window.scrollY * .055, 32)}px`);

  const readingLine = window.innerHeight * .52;
  let active = storySteps[0];
  for (const step of storySteps) {
    if (step.getBoundingClientRect().top <= readingLine) active = step;
  }
  for (const step of storySteps) step.classList.toggle('is-current', step === active);
  stage.dataset.scene = active.dataset.step;
}

function queueScrollUpdate() {
  if (!frame && !motionPreference.matches) frame = requestAnimationFrame(updateScroll);
}

window.addEventListener('scroll', queueScrollUpdate, { passive: true });
window.addEventListener('resize', queueScrollUpdate);
motionPreference.addEventListener('change', queueScrollUpdate);
queueScrollUpdate();

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
