//! Punto de entrada ("driver").
//!
//! Este módula orquesta las diferentes fases del proceso de
//! compilación y expone una CLI.

use anyhow::{self, bail, Context};
use clap::{self, crate_version, Arg};

use std::{
    fs::File,
    io::{BufRead, BufReader},
    str::FromStr,
    time::Instant,
};

use compiler::{
    error::Diagnostics,
    ir::Program,
    lex::Lexer,
    link::{LinkOptions, Linker, Platform},
    parse, source, target,
};

fn main() -> anyhow::Result<()> {
    // Parsing de CLI
    let args = clap::App::new("AnimationLed compiler")
        .version(crate_version!())
        .arg(
            Arg::new("target")
                .short('t')
                .long("target")
                .value_name("PLATFORM")
                .takes_value(true)
                .default_value("native")
                .possible_values(&["native", "esp8266"])
                .about("Target platform"),
        )
        .arg(
            Arg::new("asm")
                .short('S')
                .about("Generate assembly instead of linking"),
        )
        .arg(
            Arg::new("ir")
                .short('R')
                .long("ir")
                .about("Show IR instead of linking"),
        )
        .arg(Arg::new("strip").short('s').about("Strip executables"))
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .about("Report compilation statistics"),
        )
        .arg(
            Arg::new("output")
                .short('o')
                .takes_value(true)
                .required(true)
                .value_name("FILE")
                .about("Output file ('-' along with -S for stdout)"),
        )
        .arg(
            Arg::new("input")
                .required(true)
                .value_name("INPUT")
                .about("Input file ('-' for stdin)"),
        )
        .get_matches();

    // Se extraen argumentos necesarios
    let platform = args.value_of("target").unwrap();
    let platform = Platform::from_str(&platform).expect("main.rs allowed a bad target");
    let arch = platform.arch();
    let output = args.value_of("output").unwrap();
    let input = args.value_of("input").unwrap();

    let start_time = Instant::now();

    // Lexer->parser->magia
    let program = match input {
        "-" => {
            let stdin = std::io::stdin();
            let mut stdin = stdin.lock();

            frontend_pipeline(&mut stdin, "<stdin>")
        }

        _ => {
            let file = File::open(input)
                .with_context(|| format!("Failed to open for reading: {}", input))?;

            let mut file = BufReader::new(file);
            frontend_pipeline(&mut file, input)
        }
    };

    let program = match program {
        Ok(program) => program,

        Err(diagnostics) => {
            eprint!("{}", diagnostics);

            //FIXME
            return Ok(());
        }
    };

    if args.is_present("ir") {
        dump_ir(&program);
        return Ok(());
    }

    match (args.is_present("asm"), output) {
        // Salida a stdout sin enlazado
        (true, "-") => {
            let mut stdout = std::io::stdout();
            target::emit(&program, arch, &mut stdout).context("Failed to emit to stdin")?;
        }

        // Salida a archivo sin enlazado
        (true, path) => {
            let mut file = File::create(path)
                .with_context(|| format!("Failed to open for writing: {}", path))?;

            target::emit(&program, arch, &mut file)
                .with_context(|| format!("Failed to emit to file: {}", path))?;
        }

        // Salida a stdout con enlazado
        (false, "-") => bail!("Refusing to write executable to stdout"),

        // Salida a archivo con enlazado
        (false, path) => {
            let mut options = LinkOptions::empty();
            if args.is_present("strip") {
                options |= LinkOptions::STRIP;
            }

            let mut linker = Linker::spawn(platform, &path, options).context("Failed to link")?;
            target::emit(&program, arch, linker.stdin())
                .context("Failed to emit assembly to assembler")?;

            linker
                .finish()
                .with_context(|| format!("Failed to generate executable: {}", path))?;
        }
    };

    if args.is_present("verbose") {
        let duration = Instant::now().duration_since(start_time).as_secs_f32();
        eprintln!("Finished successful build in {:.03}s", duration);
    }

    Ok(())
}

fn frontend_pipeline<R: BufRead>(reader: &mut R, name: &str) -> Result<Program, Diagnostics> {
    let (start, stream) = source::consume(reader, name);

    let lexer = Lexer::new(start.clone(), stream);
    let tokens = match lexer.try_exhaustive() {
        Ok(tokens) => tokens,
        Err(errors) => return Err(Diagnostics::from(errors).kind("Lexical error")),
    };

    let ast = match parse::parse(tokens.iter(), start) {
        Ok(ast) => ast,
        Err(error) => return Err(Diagnostics::from(error).kind("Syntax error")),
    };

    ast.resolve()
        .map_err(|error| Diagnostics::from(error).kind("Semantic error"))
}

fn dump_ir(ir: &Program) {
    for global in ir.globals.iter() {
        println!("[GLOBAL {}]", global.as_ref());
    }

    for procedure in ir.code.iter() {
        println!("[PROC {}/{}]", procedure.name, procedure.parameters);
        for (i, instruction) in procedure.body.iter().enumerate() {
            println!("\t{:03x} {:?}", i, instruction);
        }
    }
}
