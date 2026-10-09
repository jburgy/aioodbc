Contributing
============

Thanks for your interest in contributing to ``aioodbc``, there are multiple
ways and places you can contribute.

Reporting an Issue
------------------
If you have found issue with `aioodbc` please do
not hesitate to file an issue on the GitHub_ project. When filing your
issue please make sure you can express the issue with a reproducible test
case.

When reporting an issue we also need as much information about your environment
that you can include. We never know what information will be pertinent when
trying narrow down the issue. Please include at least the following
information:

* Version of `aioodbc` and `python`.
* Version of your ODBC database
* Version of database ODBC driver
* Version of unixODBC_
* Platform you're running on (OS X, Linux, Windows).


Instructions for contributors
-----------------------------


In order to make a clone of the GitHub_ repo: open the link and press the
"Fork" button on the upper-right menu of the web page.

I hope everybody knows how to work with git and github nowadays :)

Work flow is pretty straightforward:

  1. Clone the GitHub_ repo

  2. Make a change

  3. Make sure all tests passed

  4. Commit changes to own aioodbc clone

  5. Make pull request from github page for your clone

Preconditions for running aioodbc test suite
---------------------------------------------

We expect you to use a python virtual environment plus a Rust toolchain
(stable) to run our tests.

There are several ways to make a virtual environment.

If you like to use *virtualenv* please run::

   $ cd aioodbc
   $ virtualenv --python=`which python3` venv

For standard python *venv*::

   $ cd aioodbc
   $ python3 -m venv venv

For *virtualenvwrapper*::

   $ cd aioodbc
   $ mkvirtualenv --python=`which python3` aioodbc

After that please install libraries required for development and build the
extension::

   $ pip install -r requirements-dev.txt
   $ maturin develop

Congratulations, you are ready to run the test suite


Install database
----------------
Install unixODBC and the SQLite ODBC driver (e.g. ``apt-get install
unixodbc libsqliteodbc`` or ``brew install unixodbc sqliteodbc``); the test
suite runs against SQLite by default.


Run aioodbc test suite
----------------------

After all the preconditions are met you can run tests typing the next
command::

   $ cargo fmt --check
   $ cargo clippy --all-targets -- -D warnings
   $ pytest

Or if you want to run only one particular test::

    $ pytest tests/test_aioodbc.py -k test_connect_execute_fetch

Please take a look on the produced output.

Any extra texts (print statements and so on) should be removed.


Tests coverage
--------------

We are trying hard to have good test coverage; please don't make it worse.

Use::

   $ pytest --cov=aioodbc tests/

to run the test suite and collect coverage information for the Python
glue (the compiled Rust extension itself is not covered by this).


The End
-------

After finishing all steps make a GitHub_ Pull Request, thanks.


.. _unixODBC: http://www.unixodbc.org/
.. _GitHub: https://github.com/aio-libs/aioodbc
.. _docker: https://docs.docker.com/engine/installation/
