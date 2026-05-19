const fs = require('fs');
const path = require('path');
const axios = require('axios');
const { getLanguage } = require('./configManager');

async function generateAvatar(gamePath, nickname) {
    /**
     * Отправляет запрос на http://80.85.247.83:8000/create_image
     * с никнеймом и сохраняет полученный .dat файл в gamePath/platform/
     */
    const platformDir = path.join(gamePath, 'platform');
    const avatarFilePath = path.join(platformDir, 'avatar.dat');

    // Проверяем, существует ли директория platform, если нет — создаем
    if (!fs.existsSync(platformDir)) {
        fs.mkdirSync(platformDir, { recursive: true });
    }

    try {
        // Отправляем запрос на сервер для создания аватара
        const response = await axios.get('http://80.85.247.83:8000/create_image', {
            params: { nickname },
            responseType: 'arraybuffer' // Для получения данных файла в бинарном формате
        });

        // Сохраняем файл avatar.dat
        fs.writeFileSync(avatarFilePath, response.data);
        console.log(`avatar for "${nickname}" succesfully created in ${avatarFilePath}`);
    } catch (error) {
        console.error(`failed to generate avatar with nickname "${nickname}": ${error.message}`);
        return;
    }
}

async function updateRevIni(gamePath, nickname, clanTag, launchParams) {
    /**
     * Записывает в rev.ini PlayerName, ClanTag, Параметры запуска и язык.
     */
    const revIniPath = path.join(gamePath, 'rev.ini');

    if (!fs.existsSync(revIniPath)) {
        throw new Error(`Файл rev.ini не найден по пути: ${revIniPath}`);
    }

    let lines;
    try {
        const fileContent = fs.readFileSync(revIniPath, { encoding: 'utf8' });
        lines = fileContent.split(/\r?\n/);
    } catch (err) {
        throw new Error(`Ошибка чтения файла rev.ini: ${err.message}`);
    }

    const procNameDefault = 'zhekarikstrike.exe -steam';
    const language = getLanguage(); // Получаем текущий язык
    const languageValue = language === 'ru' ? 'Russian' : 'English'; // Выбираем значение языка

    for (let i = 0; i < lines.length; i++) {
        let line = lines[i];
        if (line.startsWith('PlayerName=')) {
            lines[i] = `PlayerName=${nickname}`;
        } else if (line.startsWith('ClanTag=')) {
            lines[i] = `ClanTag=${clanTag}`;
        } else if (line.startsWith('ProcName=')) {
            lines[i] = `ProcName=${procNameDefault} ${launchParams}`;
        } else if (line.startsWith('Language = ')) {
            lines[i] = `Language = ${languageValue}`; // Обновляем язык
        }
    }

    try {
        fs.writeFileSync(revIniPath, lines.join('\r\n'), { encoding: 'utf8' });
        console.log(`rev.ini обновлен: PlayerName=${nickname}, ClanTag=${clanTag}, ProcName=${procNameDefault} ${launchParams}, Language=${languageValue}`);
    } catch (err) {
        throw new Error(`Ошибка записи файла rev.ini: ${err.message}`);
    }

    // Генерируем аватар для пользователя
    await generateAvatar(gamePath, nickname);
}

function readRevIni(gamePath) {
    /**
     * Читает файл rev.ini и возвращает PlayerName, ClanTag и параметры запуска (ProcName).
     */
    const revIniPath = path.join(gamePath, 'rev.ini');

    if (!fs.existsSync(revIniPath)) {
        console.warn(`Файл rev.ini не найден по пути: ${revIniPath}`);
        return { playerName: null, clanTag: null, launchParams: null };
    }

    let playerName = null;
    let clanTag = null;
    let launchParams = null;

    try {
        const fileContent = fs.readFileSync(revIniPath, 'utf8');
        const lines = fileContent.split(/\r?\n/);
        
        for (let line of lines) {
            if (line.startsWith('PlayerName=')) {
                playerName = line.split('=')[1]?.trim() || '';
            } else if (line.startsWith('ClanTag=')) {
                clanTag = line.split('=')[1]?.trim() || '';
            } else if (line.startsWith('ProcName=')) {
                const procNameValue = line.split('=')[1]?.trim() || '';
                const procNameParts = procNameValue.split(/\s+/);
                // Получаем все после "zhekarikstrike.exe -steam"
                const paramsIndex = procNameParts.indexOf('-steam') + 1;
                if (paramsIndex > 0 && paramsIndex < procNameParts.length) {
                    launchParams = procNameParts.slice(paramsIndex).join(' ');
                } else {
                    launchParams = '';
                }
            }
        }
    } catch (err) {
        throw new Error(`Ошибка чтения файла rev.ini: ${err.message}`);
    }

    return { playerName, clanTag, launchParams };
}

module.exports = { updateRevIni, readRevIni };