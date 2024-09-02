use std::borrow::Cow;
use std::iter::{self, Peekable};
use std::mem::take;
use std::path::Path;

use hashbrown::HashMap;
use once_cell::sync::Lazy;
use regex_cursor::engines::meta::Regex;
use ropey::RopeSlice;

use crate::config::{LanguageConfig, LanguageLoader};
use crate::parse::LayerUpdateFlags;
use crate::{
    byte_range_to_str, Injection, Language, Layer, LayerData, Range, Syntax,
    TREE_SITTER_MATCH_LIMIT,
};
use tree_sitter::query::UserPredicate;
use tree_sitter::{
    query, Capture, Grammar, InactiveQueryCursor, MatchedNodeIdx, Pattern, Query, QueryMatch,
    SyntaxTreeNode,
};

const SHEBANG: &str = r"#!\s*(?:\S*[/\\](?:env\s+(?:\-\S+\s+)*)?)?([^\s\.\d]+)";
static SHEBANG_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(SHEBANG).unwrap());

#[derive(Clone, Default, Debug)]
pub struct InjectionProperties {
    include_children: IncludedChildren,
    language: Option<Box<str>>,
    combined: bool,
}

#[derive(Debug, Clone)]
pub enum InjectionLanguageMarker<'a> {
    Name(Cow<'a, str>),
    Filename(Cow<'a, Path>),
    Shebang(String),
}

#[derive(Clone, Debug)]
pub struct InjectionQueryMatch<'tree> {
    include_children: IncludedChildren,
    language: Language,
    scope: Option<InjectionScope>,
    node: SyntaxTreeNode<'tree>,
    last_match: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum InjectionScope {
    Match {
        id: u32,
    },
    Pattern {
        pattern: Pattern,
        language: Language,
    },
}

#[derive(Clone, Default, Debug)]
enum IncludedChildren {
    #[default]
    None,
    All,
    Unnamed,
}

#[derive(Debug)]
pub struct InjectionsQuery {
    query: Query,
    injection_properties: HashMap<Pattern, InjectionProperties>,
    injection_content_capture: Option<Capture>,
    injection_language_capture: Option<Capture>,
    injection_filename_capture: Option<Capture>,
    injection_shebang_capture: Option<Capture>,
}

impl InjectionsQuery {
    pub fn new(
        grammar: Grammar,
        query_path: impl AsRef<Path>,
        query_text: &str,
    ) -> Result<Self, query::ParseError> {
        let mut injection_properties: HashMap<Pattern, InjectionProperties> = HashMap::new();
        let query = Query::new(grammar, query_text, query_path, |pattern, predicate| {
            match predicate {
                // "injection.include-unnamed-children"
                UserPredicate::SetProperty {
                    key: "injection.include-unnamed-children",
                    val: None,
                } => {
                    injection_properties
                        .entry(pattern)
                        .or_default()
                        .include_children = IncludedChildren::Unnamed
                }
                UserPredicate::SetProperty {
                    key: "injection.include-children",
                    val: None,
                } => {
                    injection_properties
                        .entry(pattern)
                        .or_default()
                        .include_children = IncludedChildren::All
                }
                UserPredicate::SetProperty {
                    key: "injection.language",
                    val: Some(lang),
                } => injection_properties.entry(pattern).or_default().language = Some(lang.into()),
                UserPredicate::SetProperty {
                    key: "injection.combined",
                    val: None,
                } => injection_properties.entry(pattern).or_default().combined = true,
                predicate => {
                    return Err(format!("unsupported predicate {predicate}").into());
                }
            }
            Ok(())
        })?;

        Ok(InjectionsQuery {
            injection_properties,
            injection_content_capture: query.get_capture("injection.content"),
            injection_language_capture: query.get_capture("injection.language"),
            injection_filename_capture: query.get_capture("injection.filename"),
            injection_shebang_capture: query.get_capture("injection.shebang"),
            query,
        })
    }

    fn process_match<'a, 'tree>(
        &self,
        query_match: &QueryMatch<'a, 'tree>,
        node_idx: MatchedNodeIdx,
        source: RopeSlice<'a>,
        loader: impl LanguageLoader,
    ) -> Option<InjectionQueryMatch<'tree>> {
        let properties = self
            .injection_properties
            .get(&query_match.pattern())
            .cloned()
            .unwrap_or_default();

        let mut injection_capture = None;
        let mut last_content_node = 0;
        let mut content_nodes = 0;
        for (i, matched_node) in query_match.matched_nodes().enumerate() {
            let capture = Some(matched_node.capture);
            if capture == self.injection_language_capture {
                let name = byte_range_to_str(matched_node.syntax_node.byte_range(), source);
                injection_capture = Some(InjectionLanguageMarker::Name(name));
            } else if capture == self.injection_filename_capture {
                let name = byte_range_to_str(matched_node.syntax_node.byte_range(), source);
                let path = Path::new(name.as_ref()).to_path_buf();
                injection_capture = Some(InjectionLanguageMarker::Filename(path.into()));
            } else if capture == self.injection_shebang_capture {
                let range = matched_node.syntax_node.byte_range();
                let node_slice = source.byte_slice(range.start as usize..range.end as usize);

                // some languages allow space and newlines before the actual string content
                // so a shebang could be on either the first or second line
                let lines = if let Ok(end) = node_slice.try_line_to_byte(2) {
                    node_slice.byte_slice(..end)
                } else {
                    node_slice
                };

                injection_capture = SHEBANG_REGEX
                    .captures_iter(regex_cursor::Input::new(lines))
                    .map(|cap| {
                        let cap = lines.byte_slice(cap.get_group(1).unwrap().range());
                        InjectionLanguageMarker::Shebang(cap.into())
                    })
                    .next()
            } else if capture == self.injection_content_capture {
                content_nodes += 1;

                last_content_node = i as u32;
            }
        }
        let language = injection_capture.or(properties
            .language
            .as_deref()
            .map(|name| InjectionLanguageMarker::Name(name.into())))?;

        let language = loader.load_language(&language)?;
        let scope = if properties.combined {
            Some(InjectionScope::Pattern {
                pattern: query_match.pattern(),
                language,
            })
        } else if content_nodes != 1 {
            Some(InjectionScope::Match {
                id: query_match.id(),
            })
        } else {
            None
        };

        Some(InjectionQueryMatch {
            language,
            scope,
            include_children: properties.include_children,
            node: query_match.matched_node(node_idx).syntax_node.clone(),
            last_match: last_content_node == node_idx,
        })
    }

    /// Executes the query on the given input and return an iterator of
    /// injection ranges together with their injection properties
    ///
    /// The ranges yielded by the iterator have an asecnding start range.
    /// The ranges do not overlap exactly (matches of the exact same node are
    /// resolved with normal precedance rules). However, ranges can be nested.
    /// For example:
    ///
    /// ``` no-compile
    ///   | range 2 |
    /// |   range 1  |
    /// ```
    /// is possible and will always result in iteration order [range1, range2].
    /// This case should be handeled by the calling function
    fn execute<'a>(
        &'a self,
        node: &SyntaxTreeNode<'a>,
        source: RopeSlice<'a>,
        new_precedance: bool,
        loader: &'a impl LanguageLoader,
    ) -> impl Iterator<Item = InjectionQueryMatch> + 'a {
        let mut cursor = InactiveQueryCursor::new();
        cursor.set_byte_range(0..u32::MAX);
        cursor.set_match_limit(TREE_SITTER_MATCH_LIMIT);
        let mut cursor = cursor.execute_query(&self.query, node, source);
        let injection_content_capture = self.injection_content_capture.unwrap();
        let iter = iter::from_fn(move || loop {
            let (query_match, node_idx) = cursor.next_matched_node()?;
            if query_match.matched_node(node_idx).capture != injection_content_capture {
                continue;
            }
            let Some(mat) = self.process_match(&query_match, node_idx, source, loader) else {
                query_match.remove();
                continue;
            };
            let range = query_match.matched_node(node_idx).syntax_node.byte_range();
            if mat.last_match {
                query_match.remove();
            }
            if range.is_empty() {
                continue;
            }
            break Some(mat);
        });
        let mut iter = iter.peekable();
        iter::from_fn(move || {
            let mut res = iter.next()?;
            // handle identical matches to correctly account for precedance
            while let Some(overlap) =
                iter.next_if(|mat| mat.node.byte_range() == res.node.byte_range())
            {
                if new_precedance {
                    res = overlap;
                }
            }
            Some(res)
        })
    }
}

impl Syntax {
    pub(crate) fn run_injection_query(
        &mut self,
        layer: Layer,
        edits: &[tree_sitter::InputEdit],
        source: RopeSlice<'_>,
        loader: &impl LanguageLoader,
        mut parse_layer: impl FnMut(Layer),
    ) {
        self.map_injections(layer, None, edits);
        let layer_data = &mut self.layer_mut(layer);
        let LanguageConfig {
            ref injections_query,
            new_precedance,
            ..
        } = *loader.get_config(layer_data.language);
        if injections_query.injection_content_capture.is_none() {
            return;
        }

        // work around borrow checker
        let parent_ranges = take(&mut layer_data.ranges);
        let parse_tree = layer_data.parse_tree.take().unwrap();
        let mut injections: Vec<Injection> = Vec::with_capacity(layer_data.injections.len());
        let mut old_injections = take(&mut layer_data.injections).into_iter().peekable();

        let injection_query =
            injections_query.execute(&parse_tree.root_node(), source, new_precedance, loader);

        let mut last_injection_end = 0;
        let mut combined_injections: HashMap<InjectionScope, Layer> = HashMap::with_capacity(32);
        for mat in injection_query {
            let range = mat.node.byte_range();
            let mut insert_position = injections.len();
            // if a parent node already has an injection ignore this injection
            // in theory the first condition would be enough to detect that
            // however in case the parent node does not include children it
            // is possible that one of these children is another seperate
            // injections. In these cases we cannot skip the injection
            if last_injection_end > range.start {
                if last_injection_end <= range.end
                    || injections.last().unwrap().range.start <= range.start
                {
                    // this condition is not needed but serves as fast path
                    // for common cases
                    continue;
                } else {
                    insert_position =
                        injections.partition_point(|injection| injection.range.end <= range.start);
                    if injections[insert_position].range.start < range.end {
                        continue;
                    }
                }
            }
            last_injection_end = range.end;

            let language = mat.language;
            let reused_injection =
                self.reuse_injection(language, range.clone(), &mut old_injections);
            let layer = match mat.scope {
                Some(scope @ InjectionScope::Match { .. }) if mat.last_match => {
                    combined_injections.remove(&scope).unwrap_or_else(|| {
                        self.init_injection(layer, mat.language, reused_injection.clone())
                    })
                }
                Some(scope) => *combined_injections.entry(scope).or_insert_with(|| {
                    self.init_injection(layer, mat.language, reused_injection.clone())
                }),
                None => self.init_injection(layer, mat.language, reused_injection.clone()),
            };
            let mut layer_data = self.layer_mut(layer);
            if !layer_data.flags.touched {
                layer_data.flags.touched = true;
                parse_layer(layer)
            }
            if layer_data.flags.reused {
                layer_data.flags.modified |= reused_injection.map_or(true, |injection| {
                    injection.range != range || injection.layer != layer
                });
            } else if let Some(reused_injection) = reused_injection {
                layer_data.flags.reused = true;
                layer_data.flags.modified = true;
                let reused_parse_tree = self.layer(reused_injection.layer).tree().clone();
                layer_data = self.layer_mut(layer);
                layer_data.parse_tree = Some(reused_parse_tree)
            }

            let old_len = injections.len();
            intersect_ranges(mat.include_children, mat.node, &parent_ranges, |range| {
                layer_data.ranges.push(tree_sitter::Range {
                    start_point: tree_sitter::Point { row: 0, col: 0 },
                    end_point: tree_sitter::Point { row: 0, col: 0 },
                    start_byte: range.start,
                    end_byte: range.end,
                });
                injections.push(Injection { range, layer });
            });
            if old_len != insert_position {
                let inserted = injections.len() - old_len;
                injections[insert_position..].rotate_right(inserted)
            }
        }

        let layer_data = &mut self.layer_mut(layer);
        layer_data.ranges = parent_ranges;
        layer_data.parse_tree = Some(parse_tree);
        // let ranges: Vec<_> = injections
        //     .iter()
        //     .map(|injection| {
        //         (
        //             injection.layer,
        //             source.byte_slice(injection.range.start as usize..injection.range.end as usize),
        //         )
        //     })
        //     .collect();
        // println!("{ranges:?}");
        layer_data.injections = injections;
    }

    /// maps the layers injection ranges trough edits to enable incremental reparse
    fn map_injections(
        &mut self,
        layer: Layer,
        offset: Option<i32>,
        mut edits: &[tree_sitter::InputEdit],
    ) {
        if edits.is_empty() && offset.unwrap_or(0) == 0 {
            return;
        }
        let layer_data = self.layer_mut(layer);
        let first_relevant_injection = layer_data
            .injections
            .partition_point(|injection| injection.range.end < edits[0].start_byte);
        if first_relevant_injection == layer_data.injections.len() {
            return;
        }
        let mut offset = if let Some(offset) = offset {
            let first_relevant_edit = edits.partition_point(|edit| {
                (edit.old_end_byte as i32) < (layer_data.ranges[0].end_byte as i32 - offset)
            });
            edits = &edits[first_relevant_edit..];
            offset
        } else {
            0
        };
        // injections and edits are non-overlapping and sorted so we can
        // apply edits in O(M+N) instead of O(NM)
        let mut edits = edits.iter().peekable();
        let mut injections = take(&mut layer_data.injections);
        for injection in &mut injections[first_relevant_injection..] {
            let range = &mut injection.range;
            let flags = &mut self.layer_mut(injection.layer).flags;

            while let Some(edit) = edits.next_if(|edit| edit.old_end_byte < range.start) {
                offset += edit.offset();
            }
            flags.moved = offset != 0;
            let mut mapped_start = (range.start as i32 + offset) as u32;
            if let Some(edit) = edits.next_if(|edit| edit.old_end_byte <= range.end) {
                if edit.start_byte < range.start {
                    flags.moved = true;
                    mapped_start = (edit.new_end_byte as i32 + offset) as u32;
                } else {
                    flags.modified = true;
                }
                offset += edit.offset();
                while let Some(edit) = edits.next_if(|edit| edit.old_end_byte <= range.end) {
                    offset += edit.offset();
                }
            }
            let mut mapped_end = (range.end as i32 + offset) as u32;
            if let Some(edit) = edits.peek().filter(|edit| edit.start_byte <= range.end) {
                flags.modified = true;

                if edit.start_byte < range.start {
                    mapped_start = (edit.new_end_byte as i32 + offset) as u32;
                    mapped_end = mapped_start;
                }
            }
            *range = mapped_start..mapped_end;
        }
        self.layer_mut(layer).injections = injections;
    }

    fn init_injection(
        &mut self,
        parent: Layer,
        language: Language,
        reuse: Option<Injection>,
    ) -> Layer {
        match reuse {
            Some(old_injection) => {
                let layer_data = self.layer_mut(old_injection.layer);
                debug_assert_eq!(layer_data.parent, Some(parent));
                layer_data.flags.reused = true;
                layer_data.flags.modified = true;
                layer_data.ranges.clear();
                old_injection.layer
            }
            None => {
                let layer = self.layers.insert(LayerData {
                    language,
                    parse_tree: None,
                    ranges: Vec::new(),
                    injections: Vec::new(),
                    flags: LayerUpdateFlags::default(),
                    parent: Some(parent),
                });
                Layer(layer as u32)
            }
        }
    }

    // TODO: only reuse if same pattern is matched
    fn reuse_injection(
        &mut self,
        language: Language,
        new_range: Range,
        injections: &mut Peekable<impl Iterator<Item = Injection>>,
    ) -> Option<Injection> {
        loop {
            let skip = injections.next_if(|injection| injection.range.end <= new_range.start);
            if skip.is_none() {
                break;
            }
        }
        let injection = injections.next_if(|injection| {
            injection.range.start < new_range.end
                && self.layer(injection.layer).language == language
                && !self.layer(injection.layer).flags.reused
        })?;
        Some(injection.clone())
    }
}

fn intersect_ranges(
    include_children: IncludedChildren,
    node: SyntaxTreeNode,
    parent_ranges: &[tree_sitter::Range],
    push_range: impl FnMut(Range),
) {
    let range = node.byte_range();
    let i = parent_ranges.partition_point(|parent_range| parent_range.end_byte <= range.start);
    let parent_ranges = parent_ranges[i..]
        .iter()
        .map(|range| range.start_byte..range.end_byte);
    match include_children {
        IncludedChildren::None => intersect_ranges_impl(
            range,
            node.children().map(|node| node.byte_range()),
            parent_ranges,
            push_range,
        ),
        IncludedChildren::All => {
            intersect_ranges_impl(range, [].into_iter(), parent_ranges, push_range)
        }
        IncludedChildren::Unnamed => intersect_ranges_impl(
            range,
            node.children()
                .filter(|node| node.is_named())
                .map(|node| node.byte_range()),
            parent_ranges,
            push_range,
        ),
    }
}

fn intersect_ranges_impl(
    range: Range,
    excluded_ranges: impl Iterator<Item = Range>,
    parent_ranges: impl Iterator<Item = Range>,
    mut push_range: impl FnMut(Range),
) {
    let mut start = range.start;
    let mut excluded_ranges = excluded_ranges.filter(|range| !range.is_empty()).peekable();
    let mut parent_ranges = parent_ranges.peekable();
    loop {
        let parent_range = parent_ranges.peek().unwrap().clone();
        if let Some(excluded_range) =
            excluded_ranges.next_if(|range| range.start <= parent_range.end)
        {
            if excluded_range.start >= range.end {
                break;
            }
            if start != excluded_range.start {
                push_range(start..excluded_range.start)
            }
            start = excluded_range.end;
        } else {
            parent_ranges.next();
            if parent_range.end >= range.end {
                break;
            }
            if start != parent_range.end {
                push_range(start..parent_range.end)
            }
            let Some(next_parent_range) = parent_ranges.peek() else {
                break;
            };
            start = next_parent_range.start;
        }
    }
    if start != range.end {
        push_range(start..range.end)
    }
}
