// ᚱ Floating Rune Particles + Scroll Animations

(function () {
    'use strict';

    // === Floating Rune Particles ===
    const canvas = document.getElementById('rune-particles');
    if (!canvas) return;
    const ctx = canvas.getContext('2d');

    const RUNES = 'ᚠᚢᚦᚨᚱᚲᚷᚹᚺᚾᛁᛃᛇᛈᛉᛊᛏᛒᛖᛗᛚᛝᛞᛟ';
    const PARTICLE_COUNT = 35;
    let particles = [];
    let W, H;

    function resize() {
        W = canvas.width = window.innerWidth;
        H = canvas.height = window.innerHeight;
    }

    function createParticle() {
        return {
            x: Math.random() * W,
            y: Math.random() * H,
            vx: (Math.random() - 0.5) * 0.3,
            vy: -Math.random() * 0.4 - 0.1,
            char: RUNES[Math.floor(Math.random() * RUNES.length)],
            size: Math.random() * 18 + 12,
            alpha: 0,
            targetAlpha: Math.random() * 0.25 + 0.05,
            fadeSpeed: Math.random() * 0.005 + 0.002,
            life: 0,
            maxLife: Math.random() * 600 + 300,
        };
    }

    function init() {
        resize();
        particles = [];
        for (let i = 0; i < PARTICLE_COUNT; i++) {
            const p = createParticle();
            p.life = Math.random() * p.maxLife; // stagger
            p.alpha = p.targetAlpha * (p.life / p.maxLife);
            particles.push(p);
        }
    }

    function draw() {
        ctx.clearRect(0, 0, W, H);
        for (const p of particles) {
            p.x += p.vx;
            p.y += p.vy;
            p.life++;

            // Fade in/out
            const lifeRatio = p.life / p.maxLife;
            if (lifeRatio < 0.2) {
                p.alpha = p.targetAlpha * (lifeRatio / 0.2);
            } else if (lifeRatio > 0.8) {
                p.alpha = p.targetAlpha * ((1 - lifeRatio) / 0.2);
            } else {
                p.alpha = p.targetAlpha;
            }

            // Reset if off-screen or expired
            if (p.life >= p.maxLife || p.y < -50 || p.x < -50 || p.x > W + 50) {
                Object.assign(p, createParticle());
                p.y = H + 20;
            }

            ctx.save();
            ctx.globalAlpha = p.alpha;
            ctx.fillStyle = `hsl(${270 + Math.sin(p.life * 0.01) * 40}, 70%, 70%)`;
            ctx.font = `${p.size}px serif`;
            ctx.fillText(p.char, p.x, p.y);
            ctx.restore();
        }
        requestAnimationFrame(draw);
    }

    window.addEventListener('resize', resize);
    init();
    draw();

    // === Terminal typing animation ===
    const lines = document.querySelectorAll('.terminal-body .line');
    lines.forEach((line) => {
        const delay = parseInt(line.dataset.delay || 0, 10);
        line.style.animationDelay = delay + 'ms';
    });

    // === Scroll fade-in (IntersectionObserver) ===
    const observer = new IntersectionObserver(
        (entries) => {
            entries.forEach((entry) => {
                if (entry.isIntersecting) {
                    entry.target.classList.add('visible');
                }
            });
        },
        { threshold: 0.1 }
    );

    document.querySelectorAll('.fade-in').forEach((el) => observer.observe(el));
})();
