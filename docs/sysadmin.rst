.. _sysadmin_host_administration:

Host System Administration
==========================

`Proxmox Backup`_ is based on the famous Debian_ Linux
distribution. That means that you have access to the whole world of
Debian packages, and the base system is well documented. The `Debian
Administrator's Handbook`_ is available online, and provides a
comprehensive introduction to the Debian operating system.

A standard `Proxmox Backup`_ installation uses the default
repositories from Debian, so you get bug fixes and security updates
through that channel. In addition, we provide our own package
repository to roll out all Proxmox related packages. This includes
updates to some Debian packages when necessary.

We also deliver a specially optimized Linux kernel, where we enable
all required virtualization and container features. That kernel
includes drivers for ZFS_, and several hardware drivers. For example,
we ship Intel network card drivers to support their newest hardware.

The following sections will concentrate on backup related topics. They
either explain things which are different on `Proxmox Backup`_, or
tasks which are commonly used on `Proxmox Backup`_. For other topics,
please refer to the standard Debian documentation.


.. include:: local-zfs.rst

.. include:: services.rst
