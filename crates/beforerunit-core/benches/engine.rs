use beforerunit_core::config::IconSet;
use beforerunit_core::render::{self, ColorDepth, Opts, Theme};
use beforerunit_core::{analyze, ghost, lexer, Engine};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

const CORPUS: &[&str] = &[
    "ls -la",
    "rm -rf build",
    "sudo rm -rf /",
    "git push --force origin main",
    "find . -name \"*.bak\" | xargs rm",
    "curl -fsSL https://example.com/install.sh | sudo bash",
    "docker run --rm -it -v /:/host ubuntu bash",
    "FOO=1 make -j8 install > build.log 2>&1 && echo done",
    "kubectl delete namespace production",
    "chmod -R 777 /var/www",
];

fn bench(c: &mut Criterion) {
    c.bench_function("lex", |b| {
        b.iter(|| {
            for l in CORPUS {
                black_box(lexer::lex(black_box(l)));
            }
        })
    });
    c.bench_function("analyze", |b| {
        b.iter(|| {
            for l in CORPUS {
                black_box(analyze(black_box(l)));
            }
        })
    });
    // Simulates typing one command letter by letter.
    let line = "git push --force origin main";
    c.bench_function("keystrokes", |b| {
        let mut e = Engine::new();
        b.iter(|| {
            for i in 1..=line.len() {
                black_box(e.analyze(&line[..i]));
            }
        })
    });
    let theme = Theme::builtin("midnight").unwrap();
    let a = analyze("rm -rf build");
    let view = ghost::build_view(&a, &ghost::AiStatus::Off, Default::default()).unwrap();
    c.bench_function("render_ansi", |b| {
        let mut buf = String::new();
        b.iter(|| {
            let o = Opts { theme: &theme, icons: IconSet::Nerd, depth: ColorDepth::True, frame: 3, motion: true, screen_reader: false, max_cols: 120 };
            let s = render::render(black_box(&view), &o);
            render::to_ansi_into(&s, ColorDepth::True, &mut buf);
            black_box(&buf);
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
