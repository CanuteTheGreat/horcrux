// Horcrux Website JavaScript

document.addEventListener('DOMContentLoaded', function() {
    // Tab switching functionality
    initTabs();

    // Smooth scroll for anchor links
    initSmoothScroll();

    // Mobile menu toggle (if needed in future)
    initMobileMenu();

    // Add animation on scroll
    initScrollAnimations();
});

/**
 * Initialize tab switching for Quick Start section
 */
function initTabs() {
    const tabButtons = document.querySelectorAll('.tab-btn');
    const tabContents = document.querySelectorAll('.tab-content');

    tabButtons.forEach(button => {
        button.addEventListener('click', function() {
            const targetTab = this.getAttribute('data-tab');

            // Remove active class from all buttons and contents
            tabButtons.forEach(btn => btn.classList.remove('active'));
            tabContents.forEach(content => content.classList.remove('active'));

            // Add active class to clicked button and corresponding content
            this.classList.add('active');
            document.getElementById(targetTab).classList.add('active');
        });
    });
}

/**
 * Initialize smooth scrolling for anchor links
 */
function initSmoothScroll() {
    const links = document.querySelectorAll('a[href^="#"]');

    links.forEach(link => {
        link.addEventListener('click', function(e) {
            const href = this.getAttribute('href');

            // Only apply to actual anchor links, not empty hrefs
            if (href && href !== '#' && href.length > 1) {
                e.preventDefault();

                const targetId = href.substring(1);
                const targetElement = document.getElementById(targetId);

                if (targetElement) {
                    const navHeight = document.querySelector('.navbar').offsetHeight;
                    const targetPosition = targetElement.offsetTop - navHeight - 20;

                    window.scrollTo({
                        top: targetPosition,
                        behavior: 'smooth'
                    });
                }
            }
        });
    });
}

/**
 * Initialize mobile menu toggle
 */
function initMobileMenu() {
    // Create mobile menu button if needed
    const navbar = document.querySelector('.navbar .container');
    const navMenu = document.querySelector('.nav-menu');

    // Check if we're on mobile
    if (window.innerWidth <= 968 && navMenu) {
        // Create hamburger button
        const mobileToggle = document.createElement('button');
        mobileToggle.classList.add('mobile-toggle');
        mobileToggle.innerHTML = '☰';
        mobileToggle.setAttribute('aria-label', 'Toggle menu');

        // Insert after nav-brand
        const navBrand = document.querySelector('.nav-brand');
        if (navBrand && !document.querySelector('.mobile-toggle')) {
            navBrand.after(mobileToggle);

            // Add click handler
            mobileToggle.addEventListener('click', function() {
                navMenu.classList.toggle('mobile-active');
                this.innerHTML = navMenu.classList.contains('mobile-active') ? '✕' : '☰';
            });
        }
    }
}

/**
 * Add animations when elements scroll into view
 */
function initScrollAnimations() {
    const observerOptions = {
        threshold: 0.1,
        rootMargin: '0px 0px -50px 0px'
    };

    const observer = new IntersectionObserver(function(entries) {
        entries.forEach(entry => {
            if (entry.isIntersecting) {
                entry.target.style.opacity = '1';
                entry.target.style.transform = 'translateY(0)';
            }
        });
    }, observerOptions);

    // Elements to animate
    const animatedElements = document.querySelectorAll(
        '.diff-card, .feature-item, .stat-card, .doc-card'
    );

    animatedElements.forEach(element => {
        element.style.opacity = '0';
        element.style.transform = 'translateY(20px)';
        element.style.transition = 'opacity 0.6s ease, transform 0.6s ease';
        observer.observe(element);
    });
}

/**
 * Add active state to navbar links based on scroll position
 */
function updateActiveNavLink() {
    const sections = document.querySelectorAll('section[id]');
    const navLinks = document.querySelectorAll('.nav-menu a[href^="#"]');

    let current = '';
    const scrollY = window.pageYOffset;

    sections.forEach(section => {
        const sectionTop = section.offsetTop;
        const sectionHeight = section.offsetHeight;

        if (scrollY >= sectionTop - 200) {
            current = section.getAttribute('id');
        }
    });

    navLinks.forEach(link => {
        link.classList.remove('active');
        if (link.getAttribute('href') === `#${current}`) {
            link.classList.add('active');
        }
    });
}

// Update active nav link on scroll
window.addEventListener('scroll', updateActiveNavLink);

/**
 * Copy code to clipboard functionality
 */
function initCodeCopy() {
    const codeBlocks = document.querySelectorAll('pre code');

    codeBlocks.forEach(block => {
        const wrapper = block.parentElement;

        // Create copy button
        const copyButton = document.createElement('button');
        copyButton.classList.add('copy-button');
        copyButton.innerHTML = 'Copy';
        copyButton.setAttribute('aria-label', 'Copy code');

        // Position button
        wrapper.style.position = 'relative';
        copyButton.style.position = 'absolute';
        copyButton.style.top = '10px';
        copyButton.style.right = '10px';
        copyButton.style.padding = '0.5rem 1rem';
        copyButton.style.backgroundColor = 'var(--primary-color)';
        copyButton.style.color = 'white';
        copyButton.style.border = 'none';
        copyButton.style.borderRadius = 'var(--border-radius)';
        copyButton.style.cursor = 'pointer';
        copyButton.style.fontSize = '0.85rem';
        copyButton.style.fontWeight = '600';
        copyButton.style.transition = 'var(--transition)';

        wrapper.appendChild(copyButton);

        // Add click handler
        copyButton.addEventListener('click', async function() {
            const code = block.textContent;

            try {
                await navigator.clipboard.writeText(code);
                copyButton.innerHTML = 'Copied!';
                copyButton.style.backgroundColor = 'var(--success-color)';

                setTimeout(() => {
                    copyButton.innerHTML = 'Copy';
                    copyButton.style.backgroundColor = 'var(--primary-color)';
                }, 2000);
            } catch (err) {
                console.error('Failed to copy:', err);
                copyButton.innerHTML = 'Failed';
                copyButton.style.backgroundColor = 'var(--danger-color)';

                setTimeout(() => {
                    copyButton.innerHTML = 'Copy';
                    copyButton.style.backgroundColor = 'var(--primary-color)';
                }, 2000);
            }
        });
    });
}

// Initialize code copy buttons
initCodeCopy();

/**
 * Add parallax effect to hero section
 */
function initParallax() {
    const hero = document.querySelector('.hero');

    if (hero) {
        window.addEventListener('scroll', function() {
            const scrolled = window.pageYOffset;
            const parallaxSpeed = 0.5;

            if (scrolled < hero.offsetHeight) {
                hero.style.transform = `translateY(${scrolled * parallaxSpeed}px)`;
            }
        });
    }
}

// Initialize parallax effect
initParallax();

/**
 * Handle responsive behavior on window resize
 */
let resizeTimer;
window.addEventListener('resize', function() {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(function() {
        // Reinitialize mobile menu on resize
        const mobileToggle = document.querySelector('.mobile-toggle');
        const navMenu = document.querySelector('.nav-menu');

        if (window.innerWidth > 968 && navMenu) {
            navMenu.classList.remove('mobile-active');
            if (mobileToggle) {
                mobileToggle.remove();
            }
        } else if (window.innerWidth <= 968 && !mobileToggle) {
            initMobileMenu();
        }
    }, 250);
});

/**
 * Add terminal typing effect
 */
function initTerminalTyping() {
    const terminalLines = document.querySelectorAll('.terminal-line');

    if (terminalLines.length === 0) return;

    // Hide all lines initially
    terminalLines.forEach((line, index) => {
        line.style.opacity = '0';
    });

    // Show lines one by one with delay
    terminalLines.forEach((line, index) => {
        setTimeout(() => {
            line.style.transition = 'opacity 0.3s ease';
            line.style.opacity = '1';
        }, index * 400);
    });
}

// Initialize terminal typing effect after page load
window.addEventListener('load', initTerminalTyping);

/**
 * Easter egg: Konami code
 */
let konamiCode = [];
const konamiPattern = ['ArrowUp', 'ArrowUp', 'ArrowDown', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'ArrowLeft', 'ArrowRight', 'b', 'a'];

document.addEventListener('keydown', function(e) {
    konamiCode.push(e.key);

    if (konamiCode.length > konamiPattern.length) {
        konamiCode.shift();
    }

    if (JSON.stringify(konamiCode) === JSON.stringify(konamiPattern)) {
        // Easter egg activated!
        document.body.style.animation = 'rainbow 2s infinite';

        // Add rainbow animation
        if (!document.getElementById('rainbow-style')) {
            const style = document.createElement('style');
            style.id = 'rainbow-style';
            style.textContent = `
                @keyframes rainbow {
                    0% { filter: hue-rotate(0deg); }
                    100% { filter: hue-rotate(360deg); }
                }
            `;
            document.head.appendChild(style);
        }

        // Show message
        const message = document.createElement('div');
        message.textContent = '🎉 You found the Horcrux secret! 🎉';
        message.style.position = 'fixed';
        message.style.top = '50%';
        message.style.left = '50%';
        message.style.transform = 'translate(-50%, -50%)';
        message.style.padding = '2rem';
        message.style.background = 'linear-gradient(135deg, var(--primary-color), var(--accent-color))';
        message.style.color = 'white';
        message.style.fontSize = '2rem';
        message.style.fontWeight = 'bold';
        message.style.borderRadius = 'var(--border-radius)';
        message.style.zIndex = '10000';
        message.style.animation = 'fadeIn 0.5s';

        document.body.appendChild(message);

        setTimeout(() => {
            document.body.style.animation = '';
            message.remove();
        }, 3000);

        konamiCode = [];
    }
});

console.log('%c⚡ Horcrux', 'font-size: 24px; font-weight: bold; color: #a855f7;');
console.log('%cA Proxmox VE alternative for Gentoo Linux', 'font-size: 14px; color: #94a3b8;');
console.log('%cBuilt with Rust 🦀', 'font-size: 12px; color: #f97316;');
console.log('%cGitHub: https://github.com/CanuteTheGreat/horcrux', 'font-size: 12px; color: #06b6d4;');
