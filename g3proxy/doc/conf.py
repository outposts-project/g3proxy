# Configuration file for the Sphinx documentation builder.
#
# For the full list of built-in configuration values, see the documentation:
# https://www.sphinx-doc.org/en/master/usage/configuration.html

# -- Project information -----------------------------------------------------
# https://www.sphinx-doc.org/en/master/usage/configuration.html#project-information

project = 'g3proxy'
copyright = '2024, Zhang Jingqiang'
author = 'Zhang Jingqiang'
release = '1.9.7'

# -- General configuration ---------------------------------------------------
# https://www.sphinx-doc.org/en/master/usage/configuration.html#general-configuration

extensions = [
    "sphinx.ext.graphviz",
]

templates_path = ['_templates']
exclude_patterns = ['_build', 'Thumbs.db', '.DS_Store']

# -- Options for HTML output -------------------------------------------------
# https://www.sphinx-doc.org/en/master/usage/configuration.html#options-for-html-output

html_theme = 'alabaster'
html_static_path = ['_static']

# -- Custom Options ----------------------------------------------------------

# Set the master document, which contains the root toctree directive.
# The default changed from 'contents' to 'index' from sphinx version 2.0,
# so we need to explicitly set it in order to be compatible with old versions.
master_doc = 'index'

# Use 'svg' format for graphviz
graphviz_output_format = 'svg'
