import { getLanguage, initializeLanguage, setLanguage, t } from './i18n.js';
import { waitForE2eReady } from '../renderer/e2e.js';

function setText(selector, key) {
    const element = document.querySelector(selector);
    if (element) element.textContent = t(key);
}

function applyTranslations() {
    setText('h1', 'update_launcher_title');
    setText('.update-info', 'update_description');
    setText('#error-modal h2', 'error_title');
    setText('#error-message', 'error_message');
    setText('#error-modal-ok', 'error_modal_ok');
    setText('#error-technical summary', 'technical_details');
}

function updateActiveLanguage(language) {
    document.querySelectorAll('.language-option').forEach((option) => option.classList.remove('active'));
    document.getElementById(`language-${language}`)?.classList.add('active');
}

async function changeLanguage(language) {
    await setLanguage(language);
    applyTranslations();
    updateActiveLanguage(language);
}

document.addEventListener('DOMContentLoaded', async () => {
    await waitForE2eReady();
    await initializeLanguage();
    applyTranslations();
    updateActiveLanguage(getLanguage());
    document.getElementById('language-en')?.addEventListener('click', () => changeLanguage('en'));
    document.getElementById('language-ru')?.addEventListener('click', () => changeLanguage('ru'));
});
