ZFS on Linux
------------

ZFS is a combined file system and logical volume manager designed by
Sun Microsystems. There is no need to manually compile ZFS modules - all
packages are included.

By using ZFS, it's possible to achieve maximum enterprise features with
low budget hardware, but also high performance systems by leveraging
SSD caching or even SSD only setups. ZFS can replace cost intense
hardware raid cards by moderate CPU and memory load combined with easy
management.

General ZFS advantages

* Easy configuration and management with GUI and CLI.
* Reliable
* Protection against data corruption
* Data compression on file system level
* Snapshots
* Copy-on-write clone
* Various raid levels: RAID0, RAID1, RAID10, RAIDZ-1, RAIDZ-2 and RAIDZ-3
* Can use SSD for cache
* Self healing
* Continuous integrity checking
* Designed for high storage capacities
* Protection against data corruption
* Asynchronous replication over network
* Open Source
* Encryption

Hardware
~~~~~~~~~

ZFS depends heavily on memory, so you need at least 8GB to start. In
practice, use as much you can get for your hardware/budget. To prevent
data corruption, we recommend the use of high quality ECC RAM.

If you use a dedicated cache and/or log disk, you should use an
enterprise class SSD (e.g. Intel SSD DC S3700 Series). This can
increase the overall performance significantly.

IMPORTANT: Do not use ZFS on top of hardware controller which has its
own cache management. ZFS needs to directly communicate with disks. An
HBA adapter is the way to go, or something like LSI controller flashed
in ``IT`` mode.


ZFS Administration
~~~~~~~~~~~~~~~~~~

This section gives you some usage examples for common tasks. ZFS
itself is really powerful and provides many options. The main commands
to manage ZFS are `zfs` and `zpool`. Both commands come with great
manual pages, which can be read with:

.. code-block:: console

  # man zpool
  # man zfs

Create a new zpool
^^^^^^^^^^^^^^^^^^

To create a new pool, at least one disk is needed. The `ashift` should
have the same sector-size (2 power of `ashift`) or larger as the
underlying disk.

.. code-block:: console

  # zpool create -f -o ashift=12 <pool> <device>

Create a new pool with RAID-0
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

Minimum 1 disk

.. code-block:: console

  # zpool create -f -o ashift=12 <pool> <device1> <device2>

Create a new pool with RAID-1
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

Minimum 2 disks

.. code-block:: console

  # zpool create -f -o ashift=12 <pool> mirror <device1> <device2>

Create a new pool with RAID-10
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

Minimum 4 disks

.. code-block:: console

  # zpool create -f -o ashift=12 <pool> mirror <device1> <device2> mirror <device3> <device4>

Create a new pool with RAIDZ-1
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

Minimum 3 disks

.. code-block:: console

  # zpool create -f -o ashift=12 <pool> raidz1 <device1> <device2> <device3>

Create a new pool with RAIDZ-2
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

Minimum 4 disks

.. code-block:: console

  # zpool create -f -o ashift=12 <pool> raidz2 <device1> <device2> <device3> <device4>

Create a new pool with cache (L2ARC)
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

It is possible to use a dedicated cache drive partition to increase
the performance (use SSD).

As `<device>` it is possible to use more devices, like it's shown in
"Create a new pool with RAID*".

.. code-block:: console

  # zpool create -f -o ashift=12 <pool> <device> cache <cache_device>

Create a new pool with log (ZIL)
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

It is possible to use a dedicated cache drive partition to increase
the performance (SSD).

As `<device>` it is possible to use more devices, like it's shown in
"Create a new pool with RAID*".

.. code-block:: console

  # zpool create -f -o ashift=12 <pool> <device> log <log_device>

Add cache and log to an existing pool
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

If you have a pool without cache and log. First partition the SSD in
2 partition with `parted` or `gdisk`

.. important:: Always use GPT partition tables.

The maximum size of a log device should be about half the size of
physical memory, so this is usually quite small. The rest of the SSD
can be used as cache.

.. code-block:: console

  # zpool add -f <pool> log <device-part1> cache <device-part2>


Changing a failed device
^^^^^^^^^^^^^^^^^^^^^^^^

.. code-block:: console

  # zpool replace -f <pool> <old device> <new device>


Changing a failed bootable device
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

Depending on how Proxmox Backup was installed it is either using `grub` or `systemd-boot`
as bootloader.

The first steps of copying the partition table, reissuing GUIDs and replacing
the ZFS partition are the same. To make the system bootable from the new disk,
different steps are needed which depend on the bootloader in use.

.. code-block:: console

  # sgdisk <healthy bootable device> -R <new device>
  # sgdisk -G <new device>
  # zpool replace -f <pool> <old zfs partition> <new zfs partition>

.. NOTE:: Use the `zpool status -v` command to monitor how far the resilvering process of the new disk has progressed.

With `systemd-boot`:

.. code-block:: console

  # pve-efiboot-tool format <new disk's ESP>
  # pve-efiboot-tool init <new disk's ESP>

.. NOTE:: `ESP` stands for EFI System Partition, which is setup as partition #2 on
  bootable disks setup by the {pve} installer since version 5.4. For details, see
  xref:sysboot_systemd_boot_setup[Setting up a new partition for use as synced ESP].

With `grub`:

Usually `grub.cfg` is located in `/boot/grub/grub.cfg`

.. code-block:: console

  # grub-install <new disk>
  # grub-mkconfig -o /path/to/grub.cfg


Activate E-Mail Notification
^^^^^^^^^^^^^^^^^^^^^^^^^^^^

ZFS comes with an event daemon, which monitors events generated by the
ZFS kernel module. The daemon can also send emails on ZFS events like
pool errors. Newer ZFS packages ship the daemon in a separate package,
and you can install it using `apt-get`:

.. code-block:: console

  # apt-get install zfs-zed

To activate the daemon it is necessary to edit `/etc/zfs/zed.d/zed.rc` with your
favourite editor, and uncomment the `ZED_EMAIL_ADDR` setting:

.. code-block:: console

  ZED_EMAIL_ADDR="root"

Please note Proxmox Backup forwards mails to `root` to the email address
configured for the root user.

IMPORTANT: The only setting that is required is `ZED_EMAIL_ADDR`. All
other settings are optional.

Limit ZFS Memory Usage
^^^^^^^^^^^^^^^^^^^^^^

It is good to use at most 50 percent (which is the default) of the
system memory for ZFS ARC to prevent performance shortage of the
host. Use your preferred editor to change the configuration in
`/etc/modprobe.d/zfs.conf` and insert:

.. code-block:: console

  options zfs zfs_arc_max=8589934592

This example setting limits the usage to 8GB.

.. IMPORTANT:: If your root file system is ZFS you must update your initramfs every time this value changes:

.. code-block:: console

  # update-initramfs -u


SWAP on ZFS
^^^^^^^^^^^

Swap-space created on a zvol may generate some troubles, like blocking the
server or generating a high IO load, often seen when starting a Backup
to an external Storage.

We strongly recommend to use enough memory, so that you normally do not
run into low memory situations. Should you need or want to add swap, it is
preferred to create a partition on a physical disk and use it as swapdevice.
You can leave some space free for this purpose in the advanced options of the
installer. Additionally, you can lower the `swappiness` value. 
A good value for servers is 10:

.. code-block:: console

  # sysctl -w vm.swappiness=10

To make the swappiness persistent, open `/etc/sysctl.conf` with
an editor of your choice and add the following line:

.. code-block:: console

  vm.swappiness = 10

.. table:: Linux kernel `swappiness` parameter values
  :widths:auto

  ====================  ===============================================================
   Value                Strategy
  ====================  ===============================================================
   vm.swappiness = 0    The kernel will swap only to avoid an 'out of memory' condition
   vm.swappiness = 1    Minimum amount of swapping without disabling it entirely.
   vm.swappiness = 10   Sometimes recommended to improve performance when sufficient memory exists in a system.
   vm.swappiness = 60   The default value.
   vm.swappiness = 100  The kernel will swap aggressively.
  ====================  ===============================================================

ZFS Compression
^^^^^^^^^^^^^^^

To activate compression:
.. code-block:: console

  # zpool set compression=lz4 <pool>

We recommend using the `lz4` algorithm, since it adds very little CPU overhead.
Other algorithms such as `lzjb` and `gzip-N` (where `N` is an integer `1-9` representing
the compression ratio, 1 is fastest and 9 is best compression) are also available.
Depending on the algorithm and how compressible the data is, having compression enabled can even increase
I/O performance.

You can disable compression at any time with:
.. code-block:: console

  # zfs set compression=off <dataset>

Only new blocks will be affected by this change.

ZFS Special Device
^^^^^^^^^^^^^^^^^^

Since version 0.8.0 ZFS supports `special` devices. A `special` device in a
pool is used to store metadata, deduplication tables, and optionally small
file blocks.

A `special` device can improve the speed of a pool consisting of slow spinning
hard disks with a lot of metadata changes. For example workloads that involve
creating, updating or deleting a large number of files will benefit from the
presence of a `special` device. ZFS datasets can also be configured to store
whole small files on the `special` device which can further improve the
performance. Use fast SSDs for the `special` device.

.. IMPORTANT:: The redundancy of the `special` device should match the one of the
  pool, since the `special` device is a point of failure for the whole pool.

.. WARNING:: Adding a `special` device to a pool cannot be undone!

Create a pool with `special` device and RAID-1:

.. code-block:: console

  # zpool create -f -o ashift=12 <pool> mirror <device1> <device2> special mirror <device3> <device4>

Adding a `special` device to an existing pool with RAID-1:

.. code-block:: console

  # zpool add <pool> special mirror <device1> <device2>

ZFS datasets expose the `special_small_blocks=<size>` property. `size` can be
`0` to disable storing small file blocks on the `special` device or a power of
two in the range between `512B` to `128K`. After setting the property new file
blocks smaller than `size` will be allocated on the `special` device.

.. IMPORTANT:: If the value for `special_small_blocks` is greater than or equal to
  the `recordsize` (default `128K`) of the dataset, *all* data will be written to
  the `special` device, so be careful!

Setting the `special_small_blocks` property on a pool will change the default
value of that property for all child ZFS datasets (for example all containers
in the pool will opt in for small file blocks).

Opt in for all file smaller than 4K-blocks pool-wide:

.. code-block:: console

  # zfs set special_small_blocks=4K <pool>

Opt in for small file blocks for a single dataset:

.. code-block:: console

  # zfs set special_small_blocks=4K <pool>/<filesystem>

Opt out from small file blocks for a single dataset:

.. code-block:: console

  # zfs set special_small_blocks=0 <pool>/<filesystem>

Troubleshooting
^^^^^^^^^^^^^^^

Corrupted cachefile

In case of a corrupted ZFS cachefile, some volumes may not be mounted during
boot until mounted manually later.

For each pool, run:

.. code-block:: console

  # zpool set cachefile=/etc/zfs/zpool.cache POOLNAME

and afterwards update the `initramfs` by running:

.. code-block:: console

  # update-initramfs -u -k all

and finally reboot your node.

Sometimes the ZFS cachefile can get corrupted, and `zfs-import-cache.service`
doesn't import the pools that aren't present in the cachefile.

Another workaround to this problem is enabling the `zfs-import-scan.service`,
which searches and imports pools via device scanning (usually slower).
