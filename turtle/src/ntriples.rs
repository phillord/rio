//! Implementation of [N-Triples](https://www.w3.org/TR/n-triples/) RDF syntax

use crate::error::*;
use crate::shared::*;
use crate::utils::*;
use rio_api::model::*;
use rio_api::parser::*;
use std::io::BufRead;
use std::u8;

/// A [N-Triples](https://www.w3.org/TR/n-triples/) streaming parser.
///
/// It implements the `TripleParser` trait.
///
/// Its memory consumption is linear in the size of the longest line of the file.
/// It does not do any allocation during parsing except buffer resizing
/// if a line significantly longer than the previous is encountered.
///
///
/// Count the number of of people using `TripleParse` API:
/// ```
/// use rio_turtle::NTriplesParser;
/// use rio_api::parser::TripleParser;
/// use rio_api::model::NamedNode;
///
/// let file = b"<http://example.com/foo> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://schema.org/Person> .
/// <http://example.com/foo> <http://schema.org/name> \"Foo\" .
/// <http://example.com/bar> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://schema.org/Person> .
/// <http://example.com/bar> <http://schema.org/name> \"Bar\" .";
///
/// let rdf_type = NamedNode { iri: "http://www.w3.org/1999/02/22-rdf-syntax-ns#type" };
/// let schema_person = NamedNode { iri: "http://schema.org/Person" };
/// let mut count = 0;
/// NTriplesParser::new(file.as_ref()).unwrap().parse_all(&mut |t| {
///     if t.predicate == rdf_type && t.object == schema_person.into() {
///         count += 1;
///     }
/// }).unwrap();
/// assert_eq!(2, count)
/// ```
pub struct NTriplesParser<R: BufRead> {
    read: OneLookAheadLineByteReader<R>,
    subject_buf: Vec<u8>,
    predicate_buf: Vec<u8>,
    object_buf: Vec<u8>,
    object_annotation_buf: Vec<u8>, // datatype or language tag
}

impl<R: BufRead> NTriplesParser<R> {
    pub fn new(reader: R) -> Result<Self, TurtleError> {
        Ok(Self {
            read: OneLookAheadLineByteReader::new(reader)?,
            subject_buf: Vec::default(),
            predicate_buf: Vec::default(),
            object_buf: Vec::default(),
            object_annotation_buf: Vec::default(),
        })
    }
}

impl<R: BufRead> TripleParser for NTriplesParser<R> {
    type Error = TurtleError;

    fn parse_step(&mut self, on_triple: &mut impl FnMut(Triple) -> ()) -> Result<(), TurtleError> {
        if let Some(result) = parse_line(
            &mut self.read,
            &mut self.subject_buf,
            &mut self.predicate_buf,
            &mut self.object_buf,
            &mut self.object_annotation_buf,
        )? {
            on_triple(result);

            //We clear the buffers
            self.subject_buf.clear();
            self.predicate_buf.clear();
            self.object_buf.clear();
            self.object_annotation_buf.clear();
        }
        Ok(())
    }

    fn is_end(&self) -> bool {
        self.read.current() == EOF
    }
}

fn parse_line<'a>(
    read: &mut impl OneLookAheadLineByteRead,
    subject_buf: &'a mut Vec<u8>,
    predicate_buf: &'a mut Vec<u8>,
    object_buf: &'a mut Vec<u8>,
    object_annotation_buf: &'a mut Vec<u8>,
) -> Result<Option<Triple<'a>>, TurtleError> {
    skip_whitespace(read)?;

    let subject = match read.current() {
        EOF | b'#' | b'\r' | b'\n' => {
            skip_until_eol(read)?;
            return Ok(None);
        }
        _ => parse_named_or_blank_node(read, subject_buf)?,
    };

    skip_whitespace(read)?;

    let predicate = parse_iriref(read, predicate_buf)?;

    skip_whitespace(read)?;

    let object = parse_term(read, object_buf, object_annotation_buf)?;

    skip_whitespace(read)?;
    read.check_is_current(b'.')?;
    read.consume()?;

    skip_whitespace(read)?;
    match read.current() {
        EOF | b'#' | b'\r' | b'\n' => skip_until_eol(read)?,
        _ => read.unexpected_char_error()?,
    }

    Ok(Some(Triple {
        subject,
        predicate,
        object,
    }))
}

fn parse_term<'a>(
    read: &mut impl OneLookAheadLineByteRead,
    buffer: &'a mut Vec<u8>,
    annotation_buffer: &'a mut Vec<u8>,
) -> Result<Term<'a>, TurtleError> {
    match read.current() {
        b'<' => Ok(parse_iriref(read, buffer)?.into()),
        b'_' => Ok(parse_blank_node_label(read, buffer)?.into()),
        b'"' => Ok(parse_literal(read, buffer, annotation_buffer)?.into()),
        _ => read.unexpected_char_error(),
    }
}

fn parse_named_or_blank_node<'a>(
    read: &mut impl OneLookAheadLineByteRead,
    buffer: &'a mut Vec<u8>,
) -> Result<NamedOrBlankNode<'a>, TurtleError> {
    match read.current() {
        b'<' => Ok(parse_iriref(read, buffer)?.into()),
        b'_' => Ok(parse_blank_node_label(read, buffer)?.into()),
        _ => read.unexpected_char_error(),
    }
}

fn parse_literal<'a>(
    read: &mut impl OneLookAheadLineByteRead,
    buffer: &'a mut Vec<u8>,
    annotation_buffer: &'a mut Vec<u8>,
) -> Result<Literal<'a>, TurtleError> {
    parse_string_literal_quote(read, buffer)?;
    skip_whitespace(read)?;

    match read.current() {
        b'@' => {
            parse_langtag(read, annotation_buffer)?;
            Ok(Literal::LanguageTaggedString {
                value: to_str(read, buffer)?,
                language: to_str(read, annotation_buffer)?,
            })
        }
        b'^' => {
            read.consume()?;
            read.check_is_current(b'^')?;
            read.consume()?;
            skip_whitespace(read)?;
            Ok(Literal::Typed {
                value: to_str(read, buffer)?,
                datatype: parse_iriref(read, annotation_buffer)?,
            })
        }
        _ => Ok(Literal::Simple {
            value: to_str(read, buffer)?,
        }),
    }
}

fn skip_whitespace(read: &mut impl OneLookAheadLineByteRead) -> Result<(), TurtleError> {
    loop {
        match read.current() {
            b' ' | b'\t' => read.consume()?,
            _ => return Ok(()),
        }
    }
}

fn skip_until_eol(read: &mut impl OneLookAheadLineByteRead) -> Result<(), TurtleError> {
    loop {
        match read.current() {
            EOF => return Ok(()),
            b'\n' => {
                read.consume()?;
                return Ok(());
            }
            _ => (),
        }
        read.consume()?;
    }
}

fn parse_iriref<'a>(
    read: &mut impl OneLookAheadLineByteRead,
    buffer: &'a mut Vec<u8>,
) -> Result<NamedNode<'a>, TurtleError> {
    parse_iriref_absolute(read, buffer)?;
    Ok(NamedNode {
        iri: to_str(read, buffer)?,
    })
}
