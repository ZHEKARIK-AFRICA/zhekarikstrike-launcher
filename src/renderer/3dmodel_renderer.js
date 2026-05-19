// Создаем рендерер и устанавливаем размер
const canvas = document.getElementById('modelCanvas');
const renderer = new THREE.WebGLRenderer({ canvas: canvas, alpha: true, antialias: true });

renderer.setPixelRatio(window.devicePixelRatio);
renderer.setSize(canvas.clientWidth, canvas.clientHeight, false);

const camera = new THREE.PerspectiveCamera(40, canvas.clientWidth / canvas.clientHeight, 0.01, 10000);
camera.position.z = 3;
camera.position.y = 0;

const scene = new THREE.Scene();

// Добавляем свет
const light = new THREE.DirectionalLight(0x1e1e1e, 1.6);
light.position.set(0, 0.7, 1).normalize();
scene.add(light);

const light2 = new THREE.DirectionalLight(0x1e1e1e, 1.6);
light2.position.set(-1.5, 0, 1).normalize();
scene.add(light2);

const light3 = new THREE.DirectionalLight(0x1e1e1e, 0.1);
light3.position.set(0.2, 0.2, 1).normalize();
scene.add(light3);



const models = ['assets/3dmodel/Trollface.glb', 'assets/3dmodel/Trollface2.glb','assets/3dmodel/Trollface3.glb','assets/3dmodel/Trollface4.glb','assets/3dmodel/Trollface5.glb'];
const randomModel = models[Math.floor(Math.random() * models.length)];
const loadedModels = []; // Массив для хранения загруженных моделей
let baseRotationSpeed = 0.025; // Базовая скорость вращения
let rotationSpeed = baseRotationSpeed; // Текущая скорость вращения
let model;
let modelReplaced = false; // Флаг для отслеживания замены модели
const raycaster = new THREE.Raycaster();
const mouse = new THREE.Vector2();

// Предварительная загрузка всех моделей
function preloadModels() {
    models.forEach((modelPath, index) => {
        loader.load(modelPath, (gltf) => {
            loadedModels[index] = gltf.scene; // Сохраняем загруженную модель в массив
        }, undefined, (error) => {
            console.error(`Error loading model ${modelPath}:`, error);
        });
    });
}

// Загружаем 3D модель
const loader = new THREE.GLTFLoader();
loader.load(models[0], (gltf) => {
    model = gltf.scene;
    scene.add(model);
    
    model.position.y = 0.15;
    model.scale.set(10, 10, 10);
    
    // Перебираем все узлы модели
    model.traverse((node) => {
        if (node.isMesh) {
            node.frustumCulled = false; // Отключаем отсечение конуса видимости
            node.material.metalness = 1; // Металлический
            node.material.roughness = 0.4; // Гладкий для отражений
            node.material.needsUpdate = true; // Обновляем материал
        }
    });
    
    animate();
}, undefined, (error) => {
    console.error('An error occurred loading the model:', error);
});

// Рассчитываем центральное положение канваса
function getCanvasCenter() {
    const rect = canvas.getBoundingClientRect();
    return {
        x: rect.left + rect.width / 2,
        y: rect.top + rect.height / 2
    };
}

let mouseRotationX = 0;
let mouseRotationY = 0;
let clickRotationY = 0;
let isRotating = false;
let rotationAngle = 0;
const fullRotation = Math.PI * 2; // Полный оборот (360 градусов)

// Рассчитываем центральное положение канваса
function getCanvasCenter() {
    const rect = canvas.getBoundingClientRect();
    return {
        x: rect.left + rect.width / 2,
        y: rect.top + rect.height / 2
    };
}

// Вращение модели при движении мыши
window.addEventListener('mousemove', (event) => {
    const canvasCenter = getCanvasCenter();
    const deltaX = event.clientX - canvasCenter.x;
    const deltaY = event.clientY - canvasCenter.y;

    const rotationSpeedX = 0.001; // Скорость вращения по оси X
    const rotationSpeedY = 0.001; // Скорость вращения по оси Y

    // Обновляем вращение от мыши
    mouseRotationY = deltaX * rotationSpeedY;
    mouseRotationX = deltaY * rotationSpeedX;

    const rect = canvas.getBoundingClientRect();

    // Нормализуем координаты мыши относительно canvas
    mouse.x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
    mouse.y = -((event.clientY - rect.top) / rect.height) * 2 + 1;

    // Обновляем raycaster с позицией камеры и направлением мыши
    raycaster.setFromCamera(mouse, camera);

    // Проверяем пересечения raycaster с моделью
    const intersects = raycaster.intersectObject(model, true); // true для проверки всех вложенных объектов

    // Меняем курсор в зависимости от наличия пересечений
    if (intersects.length > 0) {
        canvas.style.cursor = 'pointer';
    } else {
        canvas.style.cursor = 'default';
    }
});

canvas.addEventListener('click', () => {
    if (!isRotating) {
        isRotating = true;
        rotationAngle = 0;
        modelReplaced = false;
    } else {
        // Увеличиваем скорость при каждом клике, но ограничиваем максимальную скорость
        rotationSpeed = Math.min(rotationSpeed + 0.01, 0.1);
    }
});


function easeInOut(t) {
    return 0.5 * (1 - Math.cos(Math.PI * t)); // Значение от 0 до 1
}

let currentModelIndex = -1; // Индекс текущей модели, изначально -1, чтобы первая модель выбиралась всегда

function loadRandomModel() {
    let randomIndex;
    do {
        randomIndex = Math.floor(Math.random() * loadedModels.length);
    } while (randomIndex === currentModelIndex); // Повторяем, пока не выберется новая модель

    currentModelIndex = randomIndex; // Обновляем индекс текущей модели

    if (model) {
        scene.remove(model); // Удаляем текущую модель из сцены
    }
    model = loadedModels[randomIndex].clone(); // Клонируем модель, чтобы не изменять оригинал
    scene.add(model);

    model.position.y = 0.15;
    model.scale.set(10, 10, 10);

    model.traverse((node) => {
        if (node.isMesh) {
            node.frustumCulled = false;
            node.material.metalness = 1;
            node.material.roughness = 0.4;
            node.material.needsUpdate = true;
        }
    });
}


preloadModels(); // Предварительно загружаем все модели
animate(); // Запускаем анимацию


// Анимационная функция
function animate() {
    requestAnimationFrame(animate);

    if (model) {
        if (isRotating) {
            rotationAngle += rotationSpeed;

            // Применяем easing к углу поворота
            const easingProgress = easeInOut(rotationAngle / fullRotation);
            clickRotationY = fullRotation * easingProgress;

            // Проверяем, если угол вращения достиг 270 градусов (3 * PI / 2 радиан)
            if (rotationAngle >= (3 * Math.PI) / 2 && rotationAngle < fullRotation && !modelReplaced) {
                loadRandomModel(); // Заменяем модель из загруженных
                modelReplaced = true; // Устанавливаем флаг, чтобы модель не менялась повторно
            }

            if (rotationAngle >= fullRotation) {
                isRotating = false;
                rotationAngle = 0;
                clickRotationY = 0;
                modelReplaced = false; // Сбрасываем флаг для следующего вращения
                rotationSpeed = baseRotationSpeed; // Сбрасываем скорость вращения до базовой
            }
        }

        // Применяем сумму вращений к модели
        model.rotation.y = clickRotationY + mouseRotationY;
        model.rotation.x = mouseRotationX;
    }

    renderer.render(scene, camera);
}

// Обработчик изменения размера окна
window.addEventListener('resize', () => {
    const width = canvas.clientWidth;
    const height = canvas.clientHeight;
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
});



