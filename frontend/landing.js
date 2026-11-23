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
