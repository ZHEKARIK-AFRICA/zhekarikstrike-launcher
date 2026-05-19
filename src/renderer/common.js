
// common.js

export function showErrorModal(message) {
    const errorModal = document.getElementById('error-modal');
    const errorMessageElement = document.getElementById('error-message');
    if (errorModal && errorMessageElement) {
        errorMessageElement.textContent = message;
        errorModal.style.display = 'block';
    }
}

export function setupErrorModal() {
    const errorModal = document.getElementById('error-modal');
    const errorModalOk = document.getElementById('error-modal-ok');

    if (errorModalOk) {
        errorModalOk.onclick = () => {
            errorModal.style.display = 'none';
        };
    }

    window.onclick = (event) => {
        if (event.target === errorModal) {
            errorModal.style.display = 'none';
        }
    };
}

export function handleError(event, message) {
    console.error('Error received:', message);
    showErrorModal(message);
}

export async function updateProgressBar(progress, stage, timeRemaining, progressBarId, statusId, infoId) {
    const progressBar = document.getElementById(progressBarId);
    const statusElement = document.getElementById(statusId);
    const progressInfo = document.getElementById(infoId);

    if (progressBar) {
        progressBar.style.width = `${progress}%`;
    }
    if (statusElement) {
        // Используем асинхронную функцию для перевода
        const translatedStage = await window.electronAPI.t(`stageMessages.${stage}`) || stage;
        statusElement.textContent = translatedStage;
    }
    if (progressInfo) {
        if (stage === 'error') {
            progressInfo.textContent = await window.electronAPI.t('stageMessages.error') || 'Произошла ошибка';
            return;
        }

        // Форматирование процента без десятичных и с добавлением 0 перед числами меньше 10
        const formattedProgress = progress < 10 ? `0${Math.floor(progress)}` : Math.floor(progress);

        if (timeRemaining != null) {
            let timeText = '';

            // Если осталось больше часа
            if (timeRemaining >= 3600) {
                const hours = Math.floor(timeRemaining / 3600);
                timeText = `${hours} ${await window.electronAPI.t('time_units.hours')}`;
            }
            // Если осталось больше минуты
            else if (timeRemaining >= 60) {
                const minutes = Math.floor(timeRemaining / 60);
                timeText = `${minutes} ${await window.electronAPI.t('time_units.minutes')}`;
            }
            // Если осталось меньше минуты, показываем секунды
            else {
                const seconds = Math.floor(timeRemaining);
                timeText = `${seconds} ${await window.electronAPI.t('time_units.seconds')}`;
            }

            // Обновление текста с прогрессом
            progressInfo.textContent = `${formattedProgress}% ${await window.electronAPI.t('time_units.remaining')}: ${timeText}`;
        } else {
            progressInfo.textContent = `${formattedProgress}%`;
        }
    }
}


export async function resetProgressUI(progressBarId, statusId, infoId, statusMessageKey = '') {
    const progressBar = document.getElementById(progressBarId);
    const statusElement = document.getElementById(statusId);
    const progressInfo = document.getElementById(infoId);

    if (progressBar) {
        progressBar.style.width = '0%';
    }
    if (statusElement) {
        // Используем window.electronAPI.t для перевода
        const translatedStatus = statusMessageKey ? await window.electronAPI.t(`stageMessages.${statusMessageKey}`) || await window.electronAPI.t(statusMessageKey) : '';
        statusElement.textContent = translatedStatus;
    }
    if (progressInfo) {
        progressInfo.textContent = '';
    }
}

export async function updateStatus(messageKey, statusId) {
    const statusElement = document.getElementById(statusId);
    if (statusElement) {
        // Используем window.electronAPI.t для перевода
        const translatedMessage = await window.electronAPI.t(`stageMessages.${messageKey}`) || await window.electronAPI.t(messageKey);
        statusElement.textContent = translatedMessage;
    }
}