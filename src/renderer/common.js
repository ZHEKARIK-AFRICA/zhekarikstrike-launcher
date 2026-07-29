import './e2e.js';

import { t } from '../localization/i18n.js';
import { errorMessage, errorPresentation } from './errors.js';

export function showErrorModal(message, technical = '') {
    const errorModal = document.getElementById('error-modal');
    const errorMessageElement = document.getElementById('error-message');
    const technicalContainer = document.getElementById('error-technical');
    const technicalMessage = document.getElementById('error-technical-message');
    if (errorModal && errorMessageElement) {
        errorMessageElement.textContent = errorMessage(message);
        errorModal.style.display = 'block';
    }
    if (technicalContainer && technicalMessage) {
        technicalMessage.textContent = technical;
        technicalContainer.hidden = !technical;
        technicalContainer.open = false;
    }
}

export function setupErrorModal() {
    const errorModal = document.getElementById('error-modal');
    const errorModalOk = document.getElementById('error-modal-ok');
    if (errorModalOk) {
        errorModalOk.onclick = () => {
            if (errorModal) errorModal.style.display = 'none';
        };
    }
    window.addEventListener('click', (event) => {
        if (event.target === errorModal) errorModal.style.display = 'none';
    });
}

export function handleError(_event, error, { contextKey } = {}) {
    const presentation = errorPresentation(error, contextKey, t);
    console.error('Error received:', presentation.technical);
    showErrorModal(presentation.friendly, presentation.technical);
}

export function updateProgressBar(progress, stage, timeRemaining, progressBarId, statusId, infoId) {
    const progressBar = document.getElementById(progressBarId);
    const statusElement = document.getElementById(statusId);
    const progressInfo = document.getElementById(infoId);
    const safeProgress = Number.isFinite(progress) ? progress : 0;

    if (progressBar) progressBar.style.width = `${safeProgress}%`;
    if (statusElement) statusElement.textContent = t(`stageMessages.${stage}`) || stage;
    if (!progressInfo) return;

    if (stage === 'error') {
        progressInfo.textContent = t('stageMessages.error');
        return;
    }

    const formattedProgress = Math.floor(safeProgress).toString().padStart(2, '0');
    if (timeRemaining == null) {
        progressInfo.textContent = `${formattedProgress}%`;
        return;
    }

    let timeText;
    if (timeRemaining >= 3600) {
        timeText = `${Math.floor(timeRemaining / 3600)} ${t('time_units.hours')}`;
    } else if (timeRemaining >= 60) {
        timeText = `${Math.floor(timeRemaining / 60)} ${t('time_units.minutes')}`;
    } else {
        timeText = `${Math.max(0, Math.floor(timeRemaining))} ${t('time_units.seconds')}`;
    }
    progressInfo.textContent = `${formattedProgress}% ${t('time_units.remaining')}: ${timeText}`;
}

export function resetProgressUI(progressBarId, statusId, infoId, statusMessageKey = '') {
    const progressBar = document.getElementById(progressBarId);
    const statusElement = document.getElementById(statusId);
    const progressInfo = document.getElementById(infoId);
    if (progressBar) progressBar.style.width = '0%';
    if (statusElement) {
        const stageKey = `stageMessages.${statusMessageKey}`;
        const translated = statusMessageKey ? t(stageKey) : '';
        statusElement.textContent = translated === stageKey ? t(statusMessageKey) : translated;
    }
    if (progressInfo) progressInfo.textContent = '';
}

export function updateStatus(messageKey, statusId) {
    const statusElement = document.getElementById(statusId);
    if (!statusElement) return;
    const stageKey = `stageMessages.${messageKey}`;
    const translated = t(stageKey);
    statusElement.textContent = translated === stageKey ? t(messageKey) : translated;
}
